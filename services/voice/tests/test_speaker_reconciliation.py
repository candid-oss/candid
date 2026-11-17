"""Recording-wide diarization regression; all voices and names here are synthetic."""
import base64
import io
import json
import threading

import pytest
np=pytest.importorskip('numpy');sf=pytest.importorskip('soundfile');pytest.importorskip('scipy')
from asr.speaker_reconciliation import checked_policy,cluster_entries,reconcile
from asr.speaker_store import SpeakerStore,VoiceRefs


def entries(people=3, repeats=6):
    result=[]
    for n in range(people*repeats):
        who=n%people
        vector=np.eye(people)[who]+np.random.default_rng(n).normal(0,.01,people)
        result.append({'source_group':f'model-restart-{n}', 'interval':[n*3,n*3+2.5], 'seconds':2.5,'vector':vector.tolist()})
    return result


def test_anonymous_three_voices_stay_three_across_restarted_labels():
    data=entries();groups,missing=cluster_entries(data)
    assert len(groups)==3 and not missing
    for group in groups:assert len({i%3 for i in group['indices']})==1
    assert len({g['key'] for g in groups})==3
    assert cluster_entries(data)==(groups,missing)


def test_single_voice_does_not_become_many_people():
    groups,missing=cluster_entries(entries(1,18))
    assert len(groups)==1 and not missing


def test_overlap_and_different_human_names_are_not_merged():
    data=[{'source_group':str(i),'interval':[0,3],'seconds':3,'vector':[1,0],'protected':str(i)} for i in range(2)]
    groups,_=cluster_entries(data,{'version':1,'preset':'merge_more','max_speakers':1})
    assert len(groups)==2


def test_unusable_short_turn_inherits_only_an_unambiguous_source_group():
    data=entries(2,3)
    data.append({'source_group':data[0]['source_group'],'interval':[30,30.4],'seconds':.4,'vector':None})
    data.append({'source_group':'unseen','interval':[31,31.2],'seconds':.2,'vector':None})
    groups,missing=cluster_entries(data)
    assert len(groups)==2 and missing==[7]
    assert any(0 in g['indices'] and 6 in g['indices'] for g in groups)


def test_invalid_policy_is_refused():
    for value in [{'version':True,'preset':'balanced','max_speakers':0},{'version':1,'preset':'x','max_speakers':0},{'version':1,'preset':'balanced','max_speakers':True}]:
        with pytest.raises(ValueError):checked_policy(value)


class Models:
    loads=0
    def load_voice(self):self.loads+=1
    def embedding(self,a):return np.array([1.,0.]) if np.mean(a)>0 else np.array([0.,1.])


def sample(value):
    buffer=io.BytesIO();sf.write(buffer,np.full(16000*3,value),16000,format='WAV',subtype='PCM_16')
    return base64.b64encode(buffer.getvalue()).decode()


def request():
    return {'scope':'SYNTHETIC','revision':'a'*64,'entries':[
        {'source_group':str(i),'interval':[i*4,i*4+3],'sample':sample(.1 if i%2==0 else -.1)} for i in range(6)]}


def test_clustering_works_with_empty_library_and_does_not_enroll(tmp_path):
    library=SpeakerStore(tmp_path);refs=VoiceRefs(tmp_path);models=Models()
    first=reconcile(request(),library,refs,models)
    second=reconcile(request(),library,refs,Models())
    assert first==second and first['speaker_count']==2 and first['unresolved_segments']==0
    assert library.people()==[] and models.loads==1
    assert 'vector' not in json.dumps(first)
    assert len(refs.vectors(set(first['voice_refs'])))==2


def test_api_rejects_browser_and_retries_after_failure(tmp_path,monkeypatch):
    from fastapi.testclient import TestClient
    from asr.app import create_app
    import asr.speaker_reconciliation as module
    class Engine:
        device='synthetic'
        def load(self):pass
    original=module.reconcile;calls=[]
    def fake(value,library,refs):
        calls.append(value)
        if len(calls)==1:raise ValueError('SYNTHETIC failure')
        return original(value,library,refs,Models())
    monkeypatch.setattr(module,'reconcile',fake)
    with TestClient(create_app(Engine(),voiceprint_path=tmp_path/'speakers.sqlite')) as client:
        assert client.post('/v1/voiceprints/reconcile',json=request(),headers={'Origin':'https://example.test'}).status_code==403
        assert client.post('/v1/voiceprints/reconcile',json=request()).status_code==400
        result=client.post('/v1/voiceprints/reconcile',json=request())
        assert result.status_code==200 and result.json()['speaker_count']==2
        assert client.get('/v1/voiceprints').json()['people']==[]
