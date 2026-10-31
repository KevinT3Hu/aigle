use std::{fs, io::Write};
use thesauros::{
    config::Config,
    error::Fault,
    library::{Library, hash, now},
};
const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4"/></rdf:RDF></x:xmpmeta>"#;
fn put(lib: &Library, bytes: &[u8]) -> thesauros::library::Photo {
    let mut temp = tempfile::Builder::new()
        .suffix(".dng")
        .tempfile_in(lib.root.join("tmp"))
        .unwrap();
    temp.write_all(bytes).unwrap();
    let id = hash(temp.path()).unwrap();
    lib.accept_upload(temp, "sample.dng", &id).unwrap()
}
#[test]
fn migrates_prototype_catalog_and_rejects_two_processes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old");
    fs::create_dir(&path).unwrap();
    let db = rusqlite::Connection::open(path.join("catalog.sqlite3")).unwrap();
    db.execute_batch(include_str!("../migrations/0001_initial.sql"))
        .unwrap();
    let id = "a".repeat(64);
    db.execute("INSERT INTO photos VALUES (?,'old.NEF','/missing/photo.NEF',10,'2020-01-01','Camera','Lens',NULL)",[&id]).unwrap();
    drop(db);
    let lib = Library::open(&path, &[]).unwrap();
    assert_eq!(lib.photo(&id).unwrap().name, "old.NEF");
    assert_eq!(
        lib.db()
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(Library::open(&path, &[]).is_err());
    drop(lib);
    let reopened = Library::open(&path, &[]).unwrap();
    assert_eq!(
        reopened.photo(&id).unwrap().camera.as_deref(),
        Some("Camera")
    );
}
#[test]
fn queued_work_survives_restart_and_respects_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data: dir.path().join("data"),
        max_queued_jobs: 1,
        ..Config::default()
    };
    let lib = Library::with_config(config.clone()).unwrap();
    let photo = put(&lib, b"one");
    let claimed = lib.claim().unwrap().unwrap();
    assert_eq!(claimed.photo_id.as_deref(), Some(photo.id.as_str()));
    let mut temp = tempfile::Builder::new()
        .suffix(".dng")
        .tempfile_in(lib.root.join("tmp"))
        .unwrap();
    temp.write_all(b"two").unwrap();
    let id = hash(temp.path()).unwrap();
    let error = lib.accept_upload(temp, "two.dng", &id).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<Fault>(),
        Some(Fault::QueueFull)
    ));
    assert_eq!(fs::read_dir(lib.root.join("originals")).unwrap().count(), 1);
    drop(lib);
    let lib = Library::with_config(config).unwrap();
    let job = lib.claim().unwrap().unwrap();
    assert_eq!(job.id, claimed.id);
    assert_eq!(job.attempts, 2);
    lib.cancel(&job.id).unwrap();
    lib.run_job(&job).unwrap();
    assert_eq!(lib.job(&job.id).unwrap().state, "cancelled");
    lib.retry(&job.id).unwrap();
    assert_eq!(lib.job(&job.id).unwrap().state, "queued");
}
#[test]
fn recovers_journaled_xmp_without_overwriting_external_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    let lib = Library::open(&path, &[]).unwrap();
    let p = put(&lib, b"original");
    let old = lib
        .write_sidecar(&p.id, "\"none\"", XMP.as_bytes())
        .unwrap();
    let packet = XMP.replace("Rating=\"4\"", "Rating=\"2\"");
    let rev = blake3::hash(packet.as_bytes()).to_hex().to_string();
    let target = thesauros::library::sidecar(&p.path);
    lib.db()
        .unwrap()
        .execute(
            "INSERT INTO sidecar_journal VALUES (?,?,?,?,?,?)",
            rusqlite::params![
                p.id,
                target.to_str().unwrap(),
                old,
                rev,
                packet.as_bytes(),
                2
            ],
        )
        .unwrap();
    drop(lib);
    let lib = Library::open(&path, &[]).unwrap();
    assert_eq!(lib.photo(&p.id).unwrap().rating, 2);
    assert_eq!(fs::read_to_string(&target).unwrap(), packet);
    lib.db()
        .unwrap()
        .execute(
            "INSERT INTO sidecar_journal VALUES (?,?,?,?,?,?)",
            rusqlite::params![
                p.id,
                target.to_str().unwrap(),
                rev,
                "future",
                XMP.as_bytes(),
                4
            ],
        )
        .unwrap();
    fs::write(&target, b"external edits").unwrap();
    drop(lib);
    assert!(Library::open(&path, &[]).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"external edits");
}
#[test]
fn sessions_and_job_errors_do_not_leak_file_paths() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("data"), &[]).unwrap();
    let p = put(&lib, b"malformed");
    let job = lib.claim().unwrap().unwrap();
    lib.run_job(&job).unwrap();
    let job = lib.job(&job.id).unwrap();
    let json = serde_json::to_string(&job).unwrap();
    assert!(!json.contains(dir.path().to_str().unwrap()));
    assert_eq!(job.state, "failed");
    assert_eq!(lib.photo(&p.id).unwrap().status, "failed");
    lib.db()
        .unwrap()
        .execute(
            "INSERT INTO sessions VALUES ('old','csrf','cred',?)",
            [now() - 1],
        )
        .unwrap();
    lib.housekeeping().unwrap();
    assert_eq!(
        lib.db()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn concurrent_uploads_deduplicate_and_sidecar_writers_cannot_lose_edits() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("data"), &[]).unwrap();
    let ids = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let lib = lib.clone();
                scope.spawn(move || put(&lib, b"same original").id)
            })
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(fs::read_dir(lib.root.join("originals")).unwrap().count(), 1);
    assert_eq!(lib.jobs().unwrap().len(), 1);
    let results = std::thread::scope(|scope| {
        let tasks: Vec<_> = (2..4)
            .map(|rating| {
                let lib = lib.clone();
                let id = ids[0].clone();
                scope.spawn(move || {
                    lib.write_sidecar(
                        &id,
                        "\"none\"",
                        XMP.replace("Rating=\"4\"", &format!("Rating=\"{rating}\""))
                            .as_bytes(),
                    )
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().any(|r| {
        r.as_ref()
            .err()
            .is_some_and(|e| matches!(e.downcast_ref::<Fault>(), Some(Fault::Conflict)))
    }));
    assert_eq!(hash(&lib.photo(&ids[0]).unwrap().path).unwrap(), ids[0]);
}
#[test]
fn recovers_an_original_persisted_before_its_catalog_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    let lib = Library::open(&path, &[]).unwrap();
    let id = blake3::hash(b"orphan").to_hex().to_string();
    fs::write(
        lib.root.join("originals").join(format!("{id}.dng")),
        b"orphan",
    )
    .unwrap();
    drop(lib);
    let lib = Library::open(&path, &[]).unwrap();
    let p = lib.photo(&id).unwrap();
    assert!(p.name.starts_with("Recovered-"));
    assert_eq!(p.status, "pending");
    assert_eq!(lib.jobs().unwrap().len(), 1);
}

#[test]
fn camera_and_lens_filters_are_exact_and_options_cover_the_catalog() {
    use thesauros::library::Search;
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path(), &[]).unwrap();
    let a = put(&lib, b"camera-a");
    let b = put(&lib, b"camera-b");
    let c = put(&lib, b"camera-c");
    for (id, camera, lens, rating) in [
        (&a.id, "Camera A", "50mm", 5),
        (&b.id, "Camera B", "50mm II", 5),
        (&c.id, "Camera A", "35mm", 3),
    ] {
        lib.db()
            .unwrap()
            .execute(
                "UPDATE photos SET camera=?1,lens=?2,rating=?3 WHERE id=?4",
                rusqlite::params![camera, lens, rating, id],
            )
            .unwrap();
    }
    let options = lib.filter_options().unwrap();
    assert_eq!(options.cameras, ["Camera A", "Camera B"]);
    assert_eq!(options.lenses, ["35mm", "50mm", "50mm II"]);
    let query = Search {
        camera: Some("Camera A".into()),
        lens: Some("50mm".into()),
        rating: Some(5),
        ..Default::default()
    };
    let photos = lib.list(&query).unwrap();
    assert_eq!(photos.len(), 1);
    assert_eq!(photos[0].id, a.id);
    assert!(
        lib.list(&Search {
            camera: Some("Camera B".into()),
            ..query
        })
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        lib.list(&Search {
            camera: Some(String::new()),
            lens: Some(String::new()),
            ..Default::default()
        })
        .unwrap()
        .len(),
        3
    );
}
