"""Versioned local demo cache and explicit-enrollment voiceprint library."""
import json
import math
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from uuid import uuid4

import numpy as np

from .speaker_models import VOICE_ID, VOICE_REV, unit


# Matching policy measured on real recordings (cosine, CAM++): with 8 s or more of a speaker's pooled
# speech, the same person scores 0.75–0.96 and different people stay below about 0.56; with 3 s or less
# the two overlap. A name is therefore applied automatically only with enough speech, a short speaker
# can at most be offered as 「可能是」, and under 3 s there is no verdict at all.
AUTO_THRESHOLD = 0.62
AUTO_SECONDS = 8.0
MAYBE_SECONDS = 3.0
MATCH_MARGIN = 0.05
# Vectors stored before speech length was recorded were means of short clips: keep the stricter bar.
LEGACY_THRESHOLD = 0.70


def now():
    return datetime.now(timezone.utc).isoformat()


class SpeakerStore:
    def __init__(self, root):
        self.root = Path(root).resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        self.path = self.root / "speakers.sqlite"
        with self.db() as db:
            version = db.execute("PRAGMA user_version").fetchone()[0]
            if version not in (0, 1):
                raise RuntimeError(f"Unsupported speaker database version: {version}")
            db.executescript("""
                CREATE TABLE IF NOT EXISTS analyses (
                  id TEXT PRIMARY KEY, result TEXT NOT NULL, created_at TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS people (
                  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE,
                  created_at TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS samples (
                  id INTEGER PRIMARY KEY, person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
                  source_hash TEXT NOT NULL, start REAL NOT NULL, end REAL NOT NULL,
                  model_id TEXT NOT NULL, model_revision TEXT NOT NULL, vector TEXT NOT NULL,
                  enrolled_at TEXT NOT NULL, confirmed INTEGER NOT NULL CHECK(confirmed=1),
                  UNIQUE(person_id,source_hash,start,end,model_id,model_revision));
                PRAGMA user_version=1;
            """)

    @contextmanager
    def db(self):
        with sqlite3.connect(self.path, timeout=30) as db:
            db.row_factory = sqlite3.Row
            db.execute("PRAGMA foreign_keys=ON")
            yield db

    def cached(self, analysis_id):
        with self.db() as db:
            row = db.execute("SELECT result FROM analyses WHERE id=?", (analysis_id,)).fetchone()
        return json.loads(row[0]) if row else None

    def save(self, result):
        with self.db() as db:
            db.execute("INSERT OR REPLACE INTO analyses VALUES(?,?,?)",
                       (result["analysis_id"], json.dumps(result), now()))

    def people(self):
        with self.db() as db:
            return [dict(r) for r in db.execute("""SELECT p.*,COUNT(s.id) sample_count FROM people p
                LEFT JOIN samples s ON s.person_id=p.id GROUP BY p.id ORDER BY p.created_at""")]

    @staticmethod
    def clean_name(name):
        name = name.strip()
        if not 1 <= len(name) <= 80:
            raise ValueError("Name must contain 1–80 characters")
        return name

    def enroll(self, analysis_id, speaker_id, name=None, person_id=None, confirmed=False, sample_limit=3,
               with_receipt=False):
        if not confirmed:
            raise ValueError("Enrollment requires explicit confirmation")
        if sample_limit not in (1, 2, 3):
            raise ValueError("Sample limit must be 1, 2, or 3")
        result = self.cached(analysis_id)
        if not result:
            raise ValueError("Analysis not found")
        speaker = next((s for s in result["speakers"] if s["speaker_id"] == speaker_id), None)
        if not speaker or not speaker["samples"]:
            raise ValueError("No clean speaker samples; use a longer, non-overlapping utterance")
        if result["voice_model"] != {"id": VOICE_ID, "revision": VOICE_REV}:
            raise ValueError("Embedding model differs; rerun analysis")
        with self.db() as db:
            if person_id:
                if not db.execute("SELECT 1 FROM people WHERE id=?", (person_id,)).fetchone():
                    raise ValueError("Person not found")
            else:
                name = self.clean_name(name or "")
                row = db.execute("SELECT id FROM people WHERE name=? COLLATE NOCASE", (name,)).fetchone()
                person_id = row[0] if row else uuid4().hex
                if not row:
                    db.execute("INSERT INTO people VALUES(?,?,?)", (person_id, name, now()))
            for sample in speaker["samples"][:sample_limit]:
                vector = unit(sample["vector"]).tolist()
                db.execute("""INSERT OR IGNORE INTO samples
                    (person_id,source_hash,start,end,model_id,model_revision,vector,enrolled_at,confirmed)
                    VALUES(?,?,?,?,?,?,?,?,1)""", (person_id, result["source_hash"], sample["start"],
                    sample["end"], VOICE_ID, VOICE_REV, json.dumps(vector), now()))
            person = dict(db.execute("""SELECT p.*,COUNT(s.id) sample_count FROM people p
                LEFT JOIN samples s ON s.person_id=p.id WHERE p.id=? GROUP BY p.id""", (person_id,)).fetchone())
        if not with_receipt:
            return person
        receipt = {"format": "candid-voiceprint-enrollment/1", "person_id": person_id,
                   "templates": [{"source_hash": result["source_hash"], "start": sample["start"],
                                  "end": sample["end"], "model_id": VOICE_ID, "model_revision": VOICE_REV}
                                 for sample in speaker["samples"][:sample_limit]]}
        return {"person": person, "receipt": receipt}

    def withdraw(self, receipt):
        """Remove only templates selected by an enrollment receipt; retain other sources."""
        if not isinstance(receipt, dict) or receipt.get("format") != "candid-voiceprint-enrollment/1":
            raise ValueError("Unsupported enrollment receipt")
        person = receipt.get("person_id")
        templates = receipt.get("templates")
        if (not isinstance(person, str) or not person or not isinstance(templates, list)
                or not 1 <= len(templates) <= 3):
            raise ValueError("Invalid enrollment receipt")
        selectors = []
        for sample in templates:
            if not isinstance(sample, dict):
                raise ValueError("Invalid template selector")
            start, end = sample.get("start"), sample.get("end")
            if (type(start) not in (int, float) or type(end) not in (int, float)
                    or not math.isfinite(start) or not math.isfinite(end) or not 0 <= start < end
                    or any(not isinstance(sample.get(k), str) or not 1 <= len(sample[k]) <= 512
                           for k in ("source_hash", "model_id", "model_revision"))):
                raise ValueError("Invalid template selector")
            selectors.append((person, sample["source_hash"], start, end,
                              sample["model_id"], sample["model_revision"]))
        with self.db() as db:
            return sum(db.execute("""DELETE FROM samples WHERE person_id=? AND source_hash=?
                       AND start=? AND end=? AND model_id=? AND model_revision=?""", args).rowcount
                       for args in selectors)

    def rename(self, person_id, name):
        with self.db() as db:
            if db.execute("UPDATE people SET name=? WHERE id=?", (self.clean_name(name), person_id)).rowcount != 1:
                raise ValueError("Person not found")

    def delete(self, person_id):
        with self.db() as db:
            if db.execute("DELETE FROM people WHERE id=?", (person_id,)).rowcount != 1:
                raise ValueError("Person not found")

    def merge(self, source_id, target_id):
        """Move confirmed templates atomically; duplicate samples remain unique."""
        if source_id == target_id:
            raise ValueError("Choose two different people")
        with self.db() as db:
            if not db.execute("SELECT 1 FROM people WHERE id=?", (target_id,)).fetchone():
                raise ValueError("Target person not found")
            # A retry after a committed merge has nothing left to move.
            if db.execute("SELECT 1 FROM people WHERE id=?", (source_id,)).fetchone():
                db.execute("""INSERT OR IGNORE INTO samples
                    (person_id,source_hash,start,end,model_id,model_revision,vector,enrolled_at,confirmed)
                    SELECT ?,source_hash,start,end,model_id,model_revision,vector,enrolled_at,confirmed
                    FROM samples WHERE person_id=?""", (target_id, source_id))
                db.execute("DELETE FROM people WHERE id=?", (source_id,))
        return next(p for p in self.people() if p["id"] == target_id)

    def forget(self, analysis_id):
        with self.db() as db:
            if not db.execute("DELETE FROM analyses WHERE id=?", (analysis_id,)).rowcount:
                raise ValueError("Analysis not found")

    def match(self, vector, threshold=None, margin=MATCH_MARGIN, seconds=None):
        """Closest library person. `seconds` is how much speech the vector was computed from; without it
        the vector is treated as a legacy short-clip mean and judged with the stricter threshold."""
        if threshold is not None and not -1 <= threshold <= 1 or not 0 <= margin <= 2:
            raise ValueError("Invalid cosine threshold or margin")
        if seconds is not None and (type(seconds) not in (int, float) or not math.isfinite(seconds) or seconds < 0):
            raise ValueError("Invalid speech length")
        if threshold is None:
            threshold = LEGACY_THRESHOLD if seconds is None else AUTO_THRESHOLD
        unknown = {"status": "unknown", "person_id": None, "name": None, "score": None, "margin": None,
                   "threshold": threshold, "seconds": seconds, "allow_auto": False,
                   "reason": "no_usable_voiceprint", "candidates": []}
        if vector is None:
            return unknown
        if seconds is not None and seconds < MAYBE_SECONDS:
            unit(vector)
            return {**unknown, "reason": "too_little_speech"}
        vector = unit(vector)
        with self.db() as db:
            rows = db.execute("""SELECT p.id,p.name,s.vector FROM samples s JOIN people p ON s.person_id=p.id
                WHERE s.model_id=? AND s.model_revision=?""", (VOICE_ID, VOICE_REV)).fetchall()
        groups = {}
        for row in rows:
            reference = unit(json.loads(row["vector"]))
            if reference.shape != vector.shape:
                raise ValueError("Embedding dimensions differ")
            entry = groups.setdefault(row["id"], {"person_id": row["id"], "name": row["name"], "scores": []})
            entry["scores"].append(float(vector @ reference))
        candidates = []
        for entry in groups.values():
            candidates.append({"person_id": entry["person_id"], "name": entry["name"],
                               "score": float(np.mean(sorted(entry["scores"], reverse=True)[:2]))})
        candidates.sort(key=lambda c: c["score"], reverse=True)
        if not candidates:
            return {**unknown, "reason": "empty_library"}
        first = candidates[0]
        gap = first["score"] - candidates[1]["score"] if len(candidates) > 1 else None
        long_enough = seconds is None or seconds >= AUTO_SECONDS
        accepted = first["score"] >= threshold and (gap is None or gap >= margin) and long_enough
        reason = ("threshold_passed" if accepted else
                  "needs_more_speech" if first["score"] >= threshold and (gap is None or gap >= margin)
                  else "below_threshold_or_ambiguous")
        return {"status": "matched" if accepted else "unknown",
                "person_id": first["person_id"] if accepted else None,
                "name": first["name"] if accepted else None, "score": first["score"], "margin": gap,
                "threshold": threshold, "seconds": seconds, "allow_auto": accepted, "reason": reason,
                "candidates": candidates[:3]}


class VoiceRefs:
    """Speaker vectors from transcribed conversations, under opaque references stored with the transcript.

    They let a recording be matched again after the library changes (a new name, more samples) without
    transcribing it again. Vectors stay in the voice service's own folder, next to the library."""

    def __init__(self, root):
        self.path = Path(root).resolve() / "voice-refs.sqlite"
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with self.db() as db:
            db.execute("""CREATE TABLE IF NOT EXISTS refs (
                ref TEXT PRIMARY KEY, vector TEXT NOT NULL, model_id TEXT NOT NULL,
                model_revision TEXT NOT NULL, created_at TEXT NOT NULL)""")
            # Speech length behind each vector; NULL for references written before it was recorded.
            if "seconds" not in {row[1] for row in db.execute("PRAGMA table_info(refs)")}:
                db.execute("ALTER TABLE refs ADD COLUMN seconds REAL")

    @contextmanager
    def db(self):
        with sqlite3.connect(self.path, timeout=30) as db:
            yield db

    def put(self, ref, vector, seconds=None):
        with self.db() as db:
            db.execute("INSERT OR REPLACE INTO refs(ref,vector,model_id,model_revision,created_at,seconds) VALUES(?,?,?,?,?,?)",
                       (ref, json.dumps(unit(vector).tolist()), VOICE_ID, VOICE_REV, now(),
                        None if seconds is None else float(seconds)))

    def entries(self, refs):
        """{ref: (vector, seconds)} for references made with the current voice model."""
        refs = list(refs)
        with self.db() as db:
            rows = db.execute(f"SELECT ref,vector,seconds FROM refs WHERE model_id=? AND model_revision=? AND ref IN ({','.join('?' * len(refs))})",
                              (VOICE_ID, VOICE_REV, *refs)).fetchall() if refs else []
        return {ref: (json.loads(vector), seconds) for ref, vector, seconds in rows}

    def vectors(self, refs):
        return {ref: vector for ref, (vector, _) in self.entries(refs).items()}

    def forget(self, refs):
        refs = list(refs)
        if not refs:
            return 0
        with self.db() as db:
            return db.execute(f"DELETE FROM refs WHERE ref IN ({','.join('?' * len(refs))})", refs).rowcount
