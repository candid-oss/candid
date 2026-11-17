"""Isolated synthetic voiceprint administration; never loads a model or user library."""
import threading

import pytest
pytest.importorskip("numpy")
pytest.importorskip("soundfile")
from fastapi.testclient import TestClient
from asr.app import create_app
from asr.speaker_store import SpeakerStore
from asr.speaker_models import VOICE_ID, VOICE_REV


class Engine:
    device = "synthetic"
    def load(self):
        pass


def prepare(tmp_path):
    store = SpeakerStore(tmp_path)
    store.save({"analysis_id": "SYNTHETIC", "source_hash": "synthetic-source",
                "voice_model": {"id": VOICE_ID, "revision": VOICE_REV},
                "speakers": [{"speaker_id": "s1", "samples": [
                    {"start": 0, "end": 2, "vector": [1, 0]}]},
                    {"speaker_id": "s2", "samples": [
                        {"start": 2, "end": 4, "vector": [0, 1]}]}]})
    return store


def test_enrollment_requires_confirmation_and_is_idempotent(tmp_path):
    store = prepare(tmp_path)
    with TestClient(create_app(Engine(), voiceprint_path=store.path)) as client:
        command = {"action": "enroll", "analysis_id": "SYNTHETIC", "speaker_id": "s1", "name": "合成人物"}
        assert client.post("/v1/voiceprints", json=command).status_code == 400
        assert not store.people()
        assert client.post("/v1/voiceprints", json=dict(command, confirmed="true")).status_code == 422
        first = client.post("/v1/voiceprints", json=dict(command, confirmed=True)).json()
        second = client.post("/v1/voiceprints", json=dict(command, confirmed=True)).json()
        assert first["person"]["id"] == second["person"]["id"]
        assert second["person"]["sample_count"] == 1
        listing = client.get("/v1/voiceprints")
        assert listing.status_code == 200
        assert "vector" not in listing.text and "source_hash" not in listing.text
        assert store.match([1, 0])["name"] == "合成人物"
        assert (store.path.stat().st_mode & 0o777) == 0o600
        assert client.get("/v1/voiceprints", headers={"Origin": "https://example.test"}).status_code == 403


def test_rename_merge_delete_and_forget_with_safe_retries(tmp_path):
    store = prepare(tmp_path)
    a = store.enroll("SYNTHETIC", "s1", name="人物甲", confirmed=True)
    b = store.enroll("SYNTHETIC", "s2", name="人物乙", confirmed=True)
    with TestClient(create_app(Engine(), voiceprint_path=store.path)) as client:
        def command(**value):
            return client.post("/v1/voiceprints", json=value)
        assert command(action="rename", person_id=a["id"], name="人物乙").status_code == 409
        assert command(action="rename", person_id=a["id"], name="人物丙").status_code == 200
        assert command(action="merge", person_id=a["id"], target_id=b["id"]).status_code == 400
        assert len(store.people()) == 2
        for _ in range(2):
            merged = command(action="merge", person_id=a["id"], target_id=b["id"], confirmed=True)
            assert merged.status_code == 200
            assert merged.json()["person"]["sample_count"] == 2
        assert len(store.people()) == 1
        assert command(action="merge", person_id=b["id"], target_id=b["id"], confirmed=True).status_code == 400
        assert command(action="delete", person_id=b["id"]).status_code == 400
        for _ in range(2):
            assert command(action="delete", person_id=b["id"], confirmed=True).status_code == 200
        assert store.match([1, 0])["status"] == "unknown"
        for _ in range(2):
            assert command(action="forget", analysis_id="SYNTHETIC", confirmed=True).status_code == 200
        assert store.cached("SYNTHETIC") is None


def test_analysis_shares_voice_admission_and_recovers_after_failure(tmp_path):
    entered, release = threading.Event(), threading.Event()
    calls = []
    def analyzer(data):
        calls.append(data)
        if len(calls) == 1:
            entered.set()
            assert release.wait(5)
            raise ValueError("SYNTHETIC failure")
        return {"analysis_id": "SYNTHETIC", "speakers": []}
    app = create_app(Engine(), voiceprint_path=tmp_path / "speakers.sqlite", voiceprint_analyzer=analyzer)
    with TestClient(app) as client:
        responses = []
        worker = threading.Thread(target=lambda: responses.append(client.post(
            "/v1/voiceprints/analyze", files={"file": ("synthetic.wav", b"SYNTHETIC")})))
        worker.start()
        try:
            assert entered.wait(5)
            assert client.post("/v1/voice", files={"file": ("x", b"SYNTHETIC")}).status_code == 429
        finally:
            release.set()
            worker.join(5)
        assert responses[0].status_code == 400
        response = client.post("/v1/voiceprints/analyze", files={"file": ("x", b"SYNTHETIC")})
        assert response.status_code == 200
        assert response.json()["format"] == "candid-voiceprint-analysis/1"
        assert client.post("/v1/voiceprints/analyze", files={"file": ("x", b"")}).status_code == 413


def test_merge_rolls_back_on_template_write_failure(tmp_path):
    import sqlite3
    store = prepare(tmp_path)
    a = store.enroll("SYNTHETIC", "s1", name="合成甲", confirmed=True)
    b = store.enroll("SYNTHETIC", "s2", name="合成乙", confirmed=True)
    with store.db() as db:
        db.execute("CREATE TRIGGER synthetic_fault BEFORE INSERT ON samples "
                   "WHEN NEW.person_id='" + b["id"] + "' "
                   "BEGIN SELECT RAISE(ABORT,'synthetic write fault'); END")
    with pytest.raises(sqlite3.IntegrityError):
        store.merge(a["id"], b["id"])
    assert [p["sample_count"] for p in store.people()] == [1, 1]
    with store.db() as db:
        db.execute("DROP TRIGGER synthetic_fault")
    assert store.merge(a["id"], b["id"])["sample_count"] == 2


def test_source_withdrawal_preserves_other_templates_and_retries_after_merge(tmp_path):
    store = prepare(tmp_path)
    other = store.cached("SYNTHETIC")
    other.update(analysis_id="SYNTHETIC-second", source_hash="SYNTHETIC-other-source")
    store.save(other)
    with TestClient(create_app(Engine(), voiceprint_path=store.path)) as client:
        def command(**value):
            r = client.post("/v1/voiceprints", json=value)
            r.raise_for_status()
            return r.json()
        first = command(action="enroll", analysis_id="SYNTHETIC", speaker_id="s1",
                        name="人物甲", confirmed=True)
        person = first["person"]["id"]
        command(action="enroll", analysis_id="SYNTHETIC-second", speaker_id="s1",
                person_id=person, confirmed=True)
        target = command(action="enroll", analysis_id="SYNTHETIC", speaker_id="s2",
                         name="人物乙", confirmed=True)["person"]["id"]
        command(action="merge", person_id=person, target_id=target, confirmed=True)
        assert store.people()[0]["sample_count"] == 3
        receipt = dict(first["receipt"], person_id=target)
        assert "vector" not in str(receipt)
        invalid = dict(receipt, templates=receipt["templates"] + [{"start": -1}])
        assert client.post("/v1/voiceprints", json={"action": "withdraw", "receipt": invalid,
                                                  "confirmed": True}).status_code == 400
        assert store.people()[0]["sample_count"] == 3
        assert client.post("/v1/voiceprints", json={"action": "withdraw", "receipt": receipt}).status_code == 400
        assert command(action="withdraw", receipt=receipt, confirmed=True)["removed_templates"] == 1
        assert command(action="withdraw", receipt=receipt, confirmed=True)["removed_templates"] == 0
        assert store.people()[0]["sample_count"] == 2
        assert "template_withdrawal" in client.get("/v1/voiceprints").json()["capabilities"]


def test_saved_conversation_vectors_rematch_after_enrollment_and_can_be_forgotten(tmp_path):
    from asr.speaker_store import VoiceRefs
    store = prepare(tmp_path)
    refs = VoiceRefs(tmp_path)
    refs.put("ref-a", [1, 0])
    refs.put("ref-b", [0, 1])
    with TestClient(create_app(Engine(), voiceprint_path=store.path)) as client:
        def call(value):
            return client.post("/v1/voiceprints", json=value).json()
        assert call({"action": "rematch", "refs": ["ref-a", "ref-b"]})["matches"] == {"ref-a": None, "ref-b": None}
        call({"action": "enroll", "analysis_id": "SYNTHETIC", "speaker_id": "s1", "name": "合成甲", "confirmed": True})
        matches = call({"action": "rematch", "refs": ["ref-a", "ref-b", "missing"]})["matches"]
        assert matches["ref-a"]["name"] == "合成甲" and matches["ref-a"]["status"] == "matched"
        assert matches["ref-b"]["status"] == "unmatched" and "missing" not in matches
        assert call({"action": "forget_refs", "refs": ["ref-a"]})["forgotten"] == 1
        assert "ref-a" not in call({"action": "rematch", "refs": ["ref-a"]})["matches"]
