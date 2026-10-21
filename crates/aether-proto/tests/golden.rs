//! Byte-for-byte comparison against the Python reference (`legacy/python/
//! protocol_mini60.py`, dumped by `legacy/python/tools/dump_golden_mini60.py`)
//! and decoding of real F75 HE replies (`fixtures/f75/`).

use aether_proto::*;
use serde_json::Value;

fn golden() -> Value {
    serde_json::from_str(include_str!("golden/mini60.json")).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn frames_eq(ours: &[Frame], theirs: &Value) {
    let theirs: Vec<Vec<u8>> =
        theirs.as_array().unwrap().iter().map(|v| unhex(v.as_str().unwrap())).collect();
    assert_eq!(ours.len(), theirs.len(), "frame count");
    for (i, (a, b)) in ours.iter().zip(&theirs).enumerate() {
        assert_eq!(a.as_slice(), b.as_slice(), "frame {i}");
        guard(a).expect("every built frame passes the guard");
    }
}

#[test]
fn read_requests_match_python() {
    let g = golden();
    frames_eq(&device_info_reads(), &g["device_info"]);
    frames_eq(&actuation_reads(), &g["actuation_reads"]);
    frames_eq(&config_reads(), &g["config_reads"]);
    frames_eq(&key_table_reads(), &g["key_table_reads"]);
    frames_eq(&custom_led_reads(), &g["custom_led_reads"]);
    frames_eq(&[light_read()], &Value::Array(vec![g["light_read"].clone()]));
}

#[test]
fn light_writes_match_python() {
    for case in golden()["light_writes"].as_array().unwrap() {
        let a = &case["args"];
        let n = |k: &str| a[k].as_u64().unwrap() as u8;
        let l = Lighting {
            mode: n("mode"),
            rgb: [n("r"), n("g"), n("b")],
            alpha: n("alpha"),
            color_mode: n("color_mode"),
            brightness: n("brightness"),
            speed: n("speed"),
            reserved: [0; 3],
        };
        assert_eq!(l.write_frame().as_slice(), unhex(case["frame"].as_str().unwrap()));
    }
}

#[test]
fn actuation_writes_match_python() {
    let g = golden();
    let w = &g["actuation_writes"];

    frames_eq(&ActTable::default().write_frames(), &w["empty"]);

    // Reset to default writes exactly the factory (all-empty) table.
    let mut t = ActTable::default();
    t.apply(&[34, 49], Actuation::Fixed { travel_mm: 2.0 });
    t.apply(&[34, 49], Actuation::Default);
    frames_eq(&t.write_frames(), &w["empty"]);

    let mut t = ActTable::default();
    t.apply(&[34], Actuation::Fixed { travel_mm: 1.50 });
    frames_eq(&t.write_frames(), &w["w_fixed_1_50"]);

    let mut t = ActTable::default();
    t.apply(&[34], Actuation::RapidTrigger { travel_mm: 1.50, press_mm: 0.30, release_mm: 0.30 });
    frames_eq(&t.write_frames(), &w["w_rt_1_50_0_30"]);

    let mut t = ActTable::default();
    for (idx, r) in g["actuation_mixed_records"].as_object().unwrap() {
        let r = r.as_array().unwrap();
        let f = |i: usize| r[i].as_f64().unwrap();
        t.set_record(
            idx.parse().unwrap(),
            ActRecord {
                flags: f(0) as u8,
                trip: mm_to_raw(f(1)),
                press: mm_to_raw(f(2)),
                release: mm_to_raw(f(3)),
                axis: f(4) as u8,
            },
        );
    }
    frames_eq(&t.write_frames(), &w["mixed"]);
}

#[test]
fn stream_frames_match_python() {
    let g = golden();
    let mut colors = [[0u8; 3]; KEY_SLOTS];
    for (i, c) in g["stream_colors"].as_array().unwrap().iter().enumerate() {
        let c = c.as_array().unwrap();
        colors[i] = [0, 1, 2].map(|k| c[k].as_u64().unwrap() as u8);
    }
    frames_eq(&stream_frames(&led_table(&colors)), &g["stream_frames"]);
    frames_eq(&custom_led_writes(&led_table(&colors)), &g["custom_led_writes"]);
    frames_eq(&[stream_clear()], &Value::Array(vec![g["stream_clear"].clone()]));
}

#[test]
fn dangerous_list_matches_python() {
    let theirs: Vec<u8> =
        golden()["dangerous_cmds"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
    assert_eq!(theirs, DANGEROUS_CMDS.to_vec());
}

// ---- real F75 HE replies captured 2026-10-21 ----

fn f75_backup() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/f75/backup.json")).unwrap()
}

#[test]
fn f75_lighting_reply_decodes_and_restores_exactly() {
    let raw: Vec<u8> = f75_backup()["light_raw"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let l = Lighting::from_reply(&raw).unwrap();
    assert_eq!((l.mode, l.rgb, l.color_mode, l.brightness, l.speed), (11, [255; 3], 1, 5, 3));
    // the restore frame carries the read block verbatim (bytes 8..22)
    assert_eq!(&l.write_frame()[8..22], &raw[8..22]);
}

#[test]
fn f75_actuation_table_is_factory_empty() {
    let t = ActTable::from_slice(&unhex(f75_backup()["act_table"].as_str().unwrap())).unwrap();
    assert!((0..KEY_SLOTS).all(|i| t.record(i).is_empty()));
}

#[test]
fn reply_matching_requires_magic_cmd_and_offset() {
    let req = actuation_reads()[1];
    let mut rep = req;
    rep[0] = REPLY_MAGIC;
    assert!(match_reply(&rep, &req).is_some());
    let mut prefixed = vec![0u8];
    prefixed.extend_from_slice(&rep);
    assert!(match_reply(&prefixed, &req).is_some());
    rep[3] ^= 1;
    assert!(match_reply(&rep, &req).is_none());
    assert!(match_reply(&req, &req).is_none(), "an echo of our own request is not a reply");
}
