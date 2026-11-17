"""Real isolated subprocesses prove that cancel releases the service's inference slot."""
import asyncio
import json
import sys
import time

import httpx
import pytest
from fastapi import FastAPI
from asr.voice_jobs import install_voice_jobs


FAKE = r'''
import json,sys,time
from pathlib import Path
root=Path(sys.argv[1]); mode=sys.argv[2]
(root/'progress.json').write_text(json.dumps({'stage':'transcribing','completed_seconds':0,'total_seconds':1,'updated_ms':123}))
if mode=='conversation':time.sleep(60)
result={'duration_seconds':1,'segments':[], 'producer':{'model_id':'SYNTHETIC','model_revision':'test','timestamp_kind':'audio_chunk','processor_version':'1'}}
(root/'result.json').write_text(json.dumps(result))
'''


def test_managed_jobs_success_progress_idempotency_rejection_and_force_cancel():
    async def run():
        gate=asyncio.Lock(); app=FastAPI()
        manager=install_voice_jobs(app,gate,command=lambda root,mode:[sys.executable,'-c',FAKE,str(root),mode])
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),base_url='http://test') as client:
            assert (await client.get('/v1/voice/capabilities',headers={'origin':'http://browser'})).status_code==403
            assert (await client.get('/v1/voice/jobs/missing')).status_code==404
            async def submit(key, mode='quick',data=b'SYNTHETIC'):
                return await client.post('/v1/voice/jobs',files={'file':('test.wav',data)},data={'mode':mode,'request_id':key})
            assert (await submit('empty',data=b'')).status_code==413
            slow=await submit('slow','conversation');assert slow.status_code==200
            key=slow.json()['id']
            assert (await submit('busy')).status_code==429
            assert (await submit('slow','conversation')).json()['id']==key
            assert (await submit('slow','quick')).status_code==409
            for _ in range(100):
                status=(await client.get('/v1/voice/jobs/'+key)).json()
                if status['stage']=='transcribing':break
                await asyncio.sleep(.02)
            assert status['stage']=='transcribing'
            started=time.monotonic();stopped=await client.delete('/v1/voice/jobs/'+key)
            assert stopped.json()['state']=='cancelled' and time.monotonic()-started<4
            assert manager.jobs[key]['process'].poll() is not None and not gate.locked()
            assert not (manager.jobs[key]['root']/'input').exists()
            assert (await client.delete('/v1/voice/jobs/'+key)).json()['state']=='cancelled'
            done=await submit('after-stop');key=done.json()['id']
            for _ in range(100):
                value=(await client.get('/v1/voice/jobs/'+key)).json()
                if value['state']!='running':break
                await asyncio.sleep(.02)
            assert value['state']=='done' and value['result']['cancellation_supported']
            assert value['result']['source_hash']
            assert not gate.locked()
        await manager.close()
    asyncio.run(run())
