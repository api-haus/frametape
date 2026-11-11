use frametape::{Annotations, Channel, Container, Event, Frame, Mark, TapeReader, TapeWriter};
use serde_json::json;
use std::path::PathBuf;

fn scratch() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.agents/scratch/tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn independent_channels_original_payload_and_annotations_survive_one_bundle() {
    let dir = scratch();
    let mut host =
        TapeWriter::create(dir.join("journal"), json!({"host":"independent-test-host"})).unwrap();
    for (id, kind) in [
        ("input", "input"),
        ("physics.external", "physics"),
        ("profile", "profile"),
    ] {
        host.register(Channel {
            id: id.into(),
            kind: kind.into(),
            schema: "example:v1".into(),
        })
        .unwrap();
    }
    for index in 0..3 {
        host.frame(&Frame {
            index,
            timestamp_ns: index * 10,
            duration_ns: 10,
            video_frame: (index != 1).then_some(index / 2),
        })
        .unwrap();
    }
    host.event(&Event {
        frame: None,
        timestamp_ns: 15,
        channel: "input".into(),
        data: json!({"key":"Space","pressed":true}),
    })
    .unwrap();
    host.event(&Event {
        frame: Some(2),
        timestamp_ns: 20,
        channel: "profile".into(),
        data: json!({"scope":"step","duration_ns":2}),
    })
    .unwrap();
    let physics = [0, 255, 42, 0, 17];
    std::fs::write(dir.join("physics.bin"), physics).unwrap();
    host.attach(
        "physics.external",
        "simulation.b3rec",
        dir.join("physics.bin"),
        "application/octet-stream",
    )
    .unwrap();
    let path = host.finish(dir.join("issue.ftape")).unwrap();
    assert!(!dir.join("journal").exists());
    let mut reader = TapeReader::open(&path).unwrap();
    assert_eq!(
        reader.read("attachments/simulation.b3rec", 10).unwrap(),
        physics
    );
    assert_eq!(
        frametape::frame_at(
            &reader.frames().unwrap(),
            reader.events().unwrap()[0].timestamp_ns
        )
        .unwrap()
        .index,
        1
    );
    assert_eq!(reader.frames().unwrap()[1].video_frame, None);
    let annotations = Annotations {
        version: 2,
        containers: vec![Container {
            start_frame: 1,
            end_frame: 2,
            marks: vec![Mark::Text {
                at: [0.2, 0.3],
                text: "unexpected stop".into(),
                size: 0.035,
                colour: [255, 72, 72],
            }],
        }],
    };
    frametape::save_annotations(&path, &annotations).unwrap();
    let mut reopened = TapeReader::open(&path).unwrap();
    assert_eq!(reopened.annotations().unwrap(), annotations);
    assert_eq!(
        reopened.read("attachments/simulation.b3rec", 10).unwrap(),
        physics
    );
    let other = dir.join("other");
    let mut independent = TapeWriter::create(other, json!({})).unwrap();
    independent
        .register(Channel {
            id: "independent.custom".into(),
            kind: "custom".into(),
            schema: "other:v2".into(),
        })
        .unwrap();
    independent
        .event(&Event {
            frame: None,
            timestamp_ns: 0,
            channel: "independent.custom".into(),
            data: json!([1, 2, 3]),
        })
        .unwrap();
    let second = independent.finish(dir.join("other.ftape")).unwrap();
    assert_eq!(TapeReader::open(second).unwrap().manifest.channels.len(), 1);
}

#[test]
fn recover_preserves_interrupted_tail_and_rejects_interior_corruption() {
    use std::io::Write;
    let dir = scratch();
    let journal = dir.join("journal");
    let mut writer = TapeWriter::create(&journal, json!({})).unwrap();
    writer
        .frame(&Frame {
            index: 0,
            timestamp_ns: 0,
            duration_ns: 10,
            video_frame: None,
        })
        .unwrap();
    writer.checkpoint().unwrap();
    drop(writer);
    std::fs::OpenOptions::new()
        .append(true)
        .open(journal.join("frames.jsonl"))
        .unwrap()
        .write_all(b"{\"index\":1")
        .unwrap();
    let output = dir.join("recovered.ftape");
    frametape::import::recover(&journal, &output).unwrap();
    let mut reader = TapeReader::open(output).unwrap();
    assert_eq!(reader.frames().unwrap().len(), 1);
    assert_eq!(
        reader.manifest.metadata["recovery"]["truncated_frame_bytes"],
        10
    );
    assert!(
        std::fs::read(journal.join("frames.jsonl"))
            .unwrap()
            .ends_with(b"{\"index\":1")
    );
    std::fs::write(journal.join("frames.jsonl"), b"broken\n{}\n").unwrap();
    assert!(frametape::import::recover(&journal, &dir.join("invalid.ftape")).is_err());
}

#[test]
fn traversal_and_invalid_annotation_ranges_do_not_replace_evidence() {
    let dir = scratch();
    let mut writer = TapeWriter::create(dir.join("journal"), json!({})).unwrap();
    writer
        .register(Channel {
            id: "custom".into(),
            kind: "custom".into(),
            schema: "test:v1".into(),
        })
        .unwrap();
    std::fs::write(dir.join("original"), b"original").unwrap();
    assert!(
        writer
            .attach("custom", "../escape", dir.join("original"), "text/plain")
            .is_err()
    );
    writer
        .frame(&Frame {
            index: 0,
            timestamp_ns: 0,
            duration_ns: 10,
            video_frame: None,
        })
        .unwrap();
    let path = writer.finish(dir.join("valid.ftape")).unwrap();
    let before = std::fs::read(&path).unwrap();
    let invalid = Annotations {
        version: 2,
        containers: vec![Container {
            start_frame: 0,
            end_frame: 1,
            marks: vec![],
        }],
    };
    assert!(frametape::save_annotations(&path, &invalid).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn recovery_carries_pending_desktop_evidence_without_claiming_completed_pictures() {
    let dir = scratch();
    let journal = dir.join("journal");
    let mut writer = TapeWriter::create(&journal, json!({"host":"desktop"})).unwrap();
    for (id, kind, schema) in [
        ("input.evdev", "input", "linux:evdev:v1"),
        ("video", "video", "video/mp4"),
    ] {
        writer
            .register(Channel {
                id: id.into(),
                kind: kind.into(),
                schema: schema.into(),
            })
            .unwrap();
    }
    writer
        .frame(&Frame {
            index: 0,
            timestamp_ns: 0,
            duration_ns: 10,
            video_frame: Some(0),
        })
        .unwrap();
    writer.checkpoint().unwrap();
    assert!(frametape::import::recover(&journal, &dir.join("active.ftape")).is_err());
    drop(writer);
    let input = Event {
        frame: None,
        timestamp_ns: 5,
        channel: "input.evdev".into(),
        data: json!({"type":1,"code":57,"value":1}),
    };
    std::fs::write(
        journal.join("input.events.jsonl"),
        format!("{}\n", serde_json::to_string(&input).unwrap()),
    )
    .unwrap();
    std::fs::write(journal.join("capture.mp4"), b"unfinished video bytes").unwrap();
    let before = std::fs::read(journal.join("frames.jsonl")).unwrap();
    let output = dir.join("recovered.ftape");
    frametape::import::recover(&journal, &output).unwrap();
    let mut reader = TapeReader::open(output).unwrap();
    assert_eq!(reader.events().unwrap()[0].data, input.data);
    assert_eq!(reader.frames().unwrap()[0].video_frame, None);
    assert_eq!(
        reader
            .read("attachments/video.unverified.mp4", 100)
            .unwrap(),
        b"unfinished video bytes"
    );
    assert_eq!(
        reader
            .read("attachments/frames.accepted.jsonl", 1000)
            .unwrap(),
        before
    );
    assert_eq!(std::fs::read(journal.join("frames.jsonl")).unwrap(), before);
    assert_eq!(reader.manifest.metadata["recovery"]["complete"], false);
}
