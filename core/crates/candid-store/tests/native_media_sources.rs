//! Source separation and stable legacy reclassification; synthetic originals only.
mod common;
use common::Fx;
use candid_store::*;
use serde_json::json;
const MIC: &str = "40000000-0000-4000-8000-000000000001";
const FILE: &str = "40000000-0000-4000-8000-000000000002";
fn upload(fx: &Fx, id: &str, source: &str) {
    let path = fx.root.join("SYNTHETIC.wav");
    std::fs::write(&path,b"RIFF\x26\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x02\0\0\0\0\0").unwrap();
    fx.store.media_command(&json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","kind":"audio","source":source,"path":path})).unwrap();
}
#[test]
fn revoking_imports_does_not_block_microphone_or_region() {
    let fx = Fx::make("isolated-media-sources");
    upload(&fx, MIC, "mac_microphone");
    upload(&fx, FILE, "import");
    let imports = fx
        .store
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == "media.import")
        .unwrap();
    fx.store
        .set_source_status(&imports.source_id, "revoked", None)
        .unwrap();
    assert!(fx
        .store
        .media_command(&json!({"action":"content","id":MIC}))
        .is_ok());
    assert!(fx
        .store
        .media_command(&json!({"action":"content","id":FILE}))
        .is_err());
    assert_eq!(
        fx.store.media_command(&json!({"action":"list"})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let image = fx.root.join("SYNTHETIC.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
    fx.store.media_command(&json!({"action":"import","id":"40000000-0000-4000-8000-000000000003","filename":"SYNTHETIC.png","kind":"image","source":"mac_region","path":image})).unwrap();
    assert!(fx
        .store
        .sources()
        .unwrap()
        .iter()
        .any(|s| s.kind == "mac.region"));
}
#[test]
fn legacy_mixed_mirror_reclassifies_in_place_without_widening_old_grants() {
    let fx = Fx::make("legacy-source-grants");
    upload(&fx, MIC, "mac_microphone");
    let mic = fx
        .store
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == "mac.microphone")
        .unwrap();
    let event = fx
        .store
        .event_id(&mic.source_id, &format!("media:{MIC}"))
        .unwrap()
        .unwrap();
    let legacy = fx
        .store
        .register_source_with(
            "media.import",
            "资料库",
            &SourcePolicy::new(true, true),
            Some("hub-media"),
            None,
            None,
            &json!({"version":1}),
            None,
        )
        .unwrap();
    fx.store
        .raw_db()
        .conn
        .execute(
            "UPDATE events SET source_id=? WHERE event_id=?",
            [&legacy, &event],
        )
        .unwrap();
    let reader = fx
        .store
        .register_client(
            "SYNTHETIC old imports",
            "http",
            &ClientScope {
                sources: vec![legacy.clone()],
                ..ClientScope::unbounded()
            },
            None,
        )
        .unwrap();
    fx.store.project_native_media().unwrap();
    assert_eq!(
        fx.store
            .event_id(&mic.source_id, &format!("media:{MIC}"))
            .unwrap()
            .unwrap(),
        event
    );
    assert!(fx
        .store
        .call_tool(
            "search",
            br#"{"q":"SYNTHETIC","attention":"all"}"#,
            &reader.client_id
        )
        .unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        fx.store
            .call_tool(
                "search",
                br#"{"q":"SYNTHETIC","attention":"all"}"#,
                &fx.client
            )
            .unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fx.store
        .set_source_status(&legacy, "revoked", None)
        .unwrap();
    assert!(fx
        .store
        .media_command(&json!({"action":"content","id":MIC}))
        .is_err());
    let next = "40000000-0000-4000-8000-000000000004";
    upload(&fx, next, "mac_microphone");
    assert!(fx
        .store
        .media_command(&json!({"action":"content","id":next}))
        .is_ok());
}

#[test]
fn system_mixture_has_independent_permissions_and_idempotent_imports() {
    let fx = Fx::make("system-mixture-source");
    const MIX: &str = "40000000-0000-4000-8000-000000000004";
    upload(&fx, MIC, "mac_microphone");
    upload(&fx, MIX, "mac_system_audio");
    upload(&fx, MIX, "mac_system_audio");
    let sources = fx.store.sources().unwrap();
    let system = sources.iter().find(|s| s.kind == "mac.system_audio").unwrap();
    let mic = sources.iter().find(|s| s.kind == "mac.microphone").unwrap();
    assert_ne!(system.source_id, mic.source_id);
    fx.store.set_source_status(&system.source_id, "revoked", None).unwrap();
    assert!(fx.store.media_command(&json!({"action":"content","id":MIX})).is_err());
    assert!(fx.store.media_command(&json!({"action":"content","id":MIC})).is_ok());
    let path = fx.root.join("SYNTHETIC.wav");
    assert!(fx.store.media_command(&json!({"action":"import","id":MIX,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_system_audio","path":path})).is_err());
    assert!(fx.store.media_command(&json!({"action":"import","id":FILE,"filename":"SYNTHETIC.png","kind":"image","source":"mac_system_audio","path":path})).is_err());
}
