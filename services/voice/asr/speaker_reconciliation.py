"""Recording-wide anonymous clustering, followed by independent library matching.

No enrollment, transcript rewriting or assumption that a model-local speaker ID is a person.
"""
from __future__ import annotations
import base64
import hashlib
import io
import json
import math

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly
from scipy.linalg import eigh
from scipy.cluster.vq import kmeans2
from math import gcd

from .speaker_models import Models, unit, VOICE_ID, VOICE_REV

VERSION = "recording-speakers/1"
PRESETS = {"conservative": .78, "balanced": .68, "merge_more": .60}
DEFAULT_POLICY = {"version": 1, "preset": "balanced", "max_speakers": 0}


def checked_policy(policy=None):
    policy = DEFAULT_POLICY.copy() if policy is None else policy
    if (not isinstance(policy, dict) or set(policy) != set(DEFAULT_POLICY)
            or type(policy["version"]) is not int or policy["version"] != 1 or policy["preset"] not in PRESETS
            or type(policy["max_speakers"]) is not int or not 0 <= policy["max_speakers"] <= 20):
        raise ValueError("Invalid recording speaker policy")
    return dict(policy)


def overlap(a, b):
    return max(0, min(a[1], b[1]) - max(a[0], b[0])) > .35


def cluster_entries(entries, policy=None):
    """Average-link clustering with a pairwise floor, overlap and human-label constraints.

    Short outliers without enough acoustic support remain unassigned, instead of creating
    extra people. Empty voice libraries do not affect this stage.
    """
    policy = checked_policy(policy)
    n = len(entries)
    vectors = {i: unit(e["vector"]) for i, e in enumerate(entries) if e.get("vector") is not None}
    if vectors and len({v.shape for v in vectors.values()}) != 1:
        raise ValueError("Embedding dimensions differ")
    clusters = [{i} for i in vectors]
    threshold = PRESETS[policy["preset"]]
    def allowed(a, b):
        protected = {entries[i].get("protected") for i in a | b} - {None, ""}
        return len(protected) <= 1 and not any(overlap(entries[i]["interval"], entries[j]["interval"]) for i in a for j in b)
    def similarity(a, b):
        scores = [float(vectors[i] @ vectors[j]) for i in a for j in b]
        weights = [min(entries[i]["seconds"], 8) * min(entries[j]["seconds"], 8) for i in a for j in b]
        return float(np.average(scores, weights=weights)), min(scores)
    spectral = len(vectors) >= 12
    if spectral:
        ids=sorted(vectors); matrix=np.asarray([vectors[i] for i in ids]); similarity_matrix=matrix @ matrix.T
        neighbours={"conservative":3,"balanced":5,"merge_more":8}[policy["preset"]]
        graph=np.zeros_like(similarity_matrix)
        for row in range(len(ids)):
            eligible=[j for j in np.argsort(similarity_matrix[row])[::-1] if j!=row and similarity_matrix[row,j]>=.30 and allowed({ids[row]},{ids[j]})]
            graph[row,eligible[:neighbours]]=1
        graph=(graph+graph.T)*.5
        values,embedding=eigh(np.diag(graph.sum(axis=1))-graph)
        cap=min(8,len(ids)-1)
        count=int(np.argmax(np.diff(values[:cap+1])))+1
        if policy["max_speakers"]: count=min(count,policy["max_speakers"])
        best=None
        for seed in range(10):
            centers,labels=kmeans2(embedding[:,:count],count,minit="++",seed=seed)
            loss=float(np.sum((embedding[:,:count]-centers[labels])**2))
            if best is None or loss<best[0]: best=(loss,labels)
        clusters=[]
        for label in sorted(set(best[1])):
            for i in [ids[j] for j in range(len(ids)) if best[1][j]==label]:
                target=next((c for c in clusters if allowed(c,{i}) and all(best[1][ids.index(j)]==label for j in c)),None)
                if target is None: clusters.append({i})
                else: target.add(i)
    while not spectral:
        candidates = [(similarity(a, b), i, j) for i, a in enumerate(clusters) for j, b in enumerate(clusters[:i]) if allowed(a, b)]
        if not candidates: break
        (score, floor), i, j = max(candidates, key=lambda x: (x[0][0], -x[1], -x[2]))
        constrained = policy["max_speakers"] > 0 and len(clusters) > policy["max_speakers"]
        if not constrained and (score < threshold or floor < threshold - .20): break
        clusters[j] |= clusters[i]; clusters.pop(i)
    # A tiny isolated cluster is not sufficient evidence for an extra person. Try a clear
    # nearest substantial cluster; retain uncertainty when two candidates are close.
    substantial = [c for c in clusters if sum(entries[i]["seconds"] for i in c) >= 3 or any(entries[i].get("protected") for i in c)]
    unresolved = set(range(n)) - set(vectors)
    for c in clusters:
        if c in substantial: continue
        options = [(similarity(c, other)[0], other) for other in substantial if allowed(c, other)]
        options.sort(key=lambda x: x[0], reverse=True)
        if options and options[0][0] >= threshold - .10 and (len(options) == 1 or options[0][0] - options[1][0] >= .06):
            options[0][1].update(c)
        else: unresolved.update(c)
    if not substantial and clusters:
        substantial = [max(clusters, key=lambda c: sum(entries[i]["seconds"] for i in c))]
        unresolved -= substantial[0]
    # Short turns can inherit their model-local label only when its reliable acoustic turns
    # overwhelmingly agree. No nearest-neighbour guess from chronological proximity.
    for i in sorted(unresolved.copy()):
        same = [j for j in vectors if entries[j].get("source_group") == entries[i].get("source_group")]
        weights = [(sum(entries[j]["seconds"] for j in c if j in same), c) for c in substantial]
        total = sum(w for w, _ in weights)
        if total > 0:
            best, c = max(weights, key=lambda x: x[0])
            if best / total >= .85 and allowed({i}, c): c.add(i); unresolved.remove(i)
    substantial.sort(key=lambda c: min(entries[i]["interval"][0] for i in c))
    result = []
    for number, c in enumerate(substantial, 1):
        anchors = [i for i in sorted(c) if i in vectors]
        weighted = np.average([vectors[i] for i in anchors], axis=0, weights=[min(8, entries[i]["seconds"]) for i in anchors])
        canonical = next(((entries[i].get("correction_key") or entries[i]["source_group"]) for i in sorted(c) if entries[i].get("protected")), entries[min(c)]["source_group"])
        result.append({"key": "recording-speaker:"+hashlib.sha256(json.dumps(sorted(c)).encode()).hexdigest()[:20], "correction_key":canonical, "label": f"说话人 {number}", "indices": sorted(c), "vector": unit(weighted).tolist()})
    return result, sorted(unresolved)


def reconcile(request, library, refs, models=None):
    policy = checked_policy(request.get("policy"))
    entries = request.get("entries")
    if (not isinstance(entries, list) or not 1 <= len(entries) <= 10000
            or not isinstance(request.get("scope"), str) or len(request["scope"]) > 128
            or not isinstance(request.get("revision"), str) or len(request["revision"]) != 64):
        raise ValueError("Invalid recording speaker request")
    models = models or Models()
    loaded = False; total = 0; decoded = []
    for entry in entries:
        if (not isinstance(entry, dict) or not isinstance(entry.get("source_group"), str)
                or len(entry["source_group"]) > 200 or not isinstance(entry.get("interval"), list)
                or len(entry["interval"]) != 2): raise ValueError("Invalid speaker interval")
        a, b = entry["interval"]
        if (any(type(t) not in (int, float) or not math.isfinite(t) for t in (a, b))
                or not 0 <= a < b <= 4 * 3600): raise ValueError("Invalid speaker interval")
        vector = None
        sample = entry.get("sample")
        if sample:
            total += len(sample)
            if not isinstance(sample, str) or total > 48 * 1024 * 1024: raise ValueError("Speaker samples too large")
            raw = base64.b64decode(sample, validate=True)
            with sf.SoundFile(io.BytesIO(raw)) as sound:
                if sound.channels > 2 or not 1.5 <= sound.frames / sound.samplerate <= 8.1: raise ValueError("Invalid speaker sample")
                audio = sound.read(dtype="float32", always_2d=True).mean(axis=1); rate = sound.samplerate
            if not np.isfinite(audio).all(): raise ValueError("Invalid speaker sample")
            if rate != 16000:
                factor = gcd(rate, 16000); audio = resample_poly(audio, 16000 // factor, rate // factor).astype(np.float32)
            if not loaded: models.load_voice(); loaded = True
            vector = models.embedding(audio)
        decoded.append({"source_group": entry["source_group"], "interval": [a,b], "seconds": b-a,
                        "protected": entry.get("protected"), "correction_key":entry.get("correction_key"), "vector": None if vector is None else unit(vector).tolist()})
    for source_group,sample in request.get("group_samples",{}).items():
        if any(e["source_group"]==source_group and e["vector"] is not None for e in decoded): continue
        total+=len(sample)
        if total>48*1024*1024: raise ValueError("Speaker samples too large")
        raw=base64.b64decode(sample,validate=True)
        with sf.SoundFile(io.BytesIO(raw)) as sound:
            if sound.channels!=1 or sound.samplerate!=16000 or not 1.5<=sound.frames/16000<=8.1: raise ValueError("Invalid pooled speaker sample")
            audio=sound.read(dtype="float32")
        if not np.isfinite(audio).all():raise ValueError("Invalid pooled speaker sample")
        if not loaded: models.load_voice();loaded=True
        vector=models.embedding(audio)
        if vector is not None:
            for entry in decoded:
                if entry["source_group"]==source_group:entry["vector"]=unit(vector).tolist()
    groups, unresolved = cluster_entries(decoded, policy)
    assignments = []
    for group in groups:
        ref = hashlib.sha256(json.dumps([VERSION, request["scope"], request["revision"], policy, group["indices"]],sort_keys=True).encode()).hexdigest()[:32]
        refs.put(ref, group["vector"])
        identity = library.match(group["vector"], threshold=.70, margin=.05)
        candidates = identity.get("candidates") or []
        match = None
        if candidates:
            top = candidates[0]
            # A new name requires agreement from the member voices, not one lucky short clip.
            votes = [(decoded[i]["seconds"], library.match(decoded[i]["vector"], threshold=.65, margin=.05)) for i in group["indices"] if decoded[i]["vector"] is not None]
            total_voice = sum(w for w,_ in votes)
            agreed = sum(w for w,m in votes if m.get("person_id")==top["person_id"] and m.get("status")=="matched")
            accepted = identity.get("status")=="matched" and (total_voice == 0 or agreed/total_voice >= .70)
            match = {"person_id":top["person_id"],"name":top["name"],"score":float(identity["score"]),"margin":identity.get("margin"),"threshold":.70,
                     "status":"matched" if accepted else "unmatched", "allow_auto":accepted}
        for i in group["indices"]:
            assignments.append({"index":i,"speaker":group["key"],"correction_key":group["correction_key"],"label":group["label"],"voice_ref":ref,"match":match})
    for i in unresolved:
        assignments.append({"index":i,"speaker":"unresolved","label":"未确定说话人","voice_ref":None,"match":None})
    return {"format":VERSION,"revision":request["revision"],"policy":policy,"assignments":sorted(assignments,key=lambda a:a["index"]),
            "speaker_count":len(groups),"unresolved_segments":len(unresolved),"producer":{"model_id":VOICE_ID,"model_revision":VOICE_REV,"processor_version":VERSION},
            "voice_refs":[a["voice_ref"] for a in assignments if a["voice_ref"]]}
