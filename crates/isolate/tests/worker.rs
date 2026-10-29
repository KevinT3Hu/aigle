//! Worker processes end to end: this test binary is its own worker (`harness = false`, so
//! `main` can hand over to `serve`).

use std::time::{Duration, Instant};

use aigle_engine_api::CancelToken;
use aigle_isolate::{Call, Error, Event, Failure, Reply, Task, call};

fn echo(body: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    let s: String = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let n = s.len();
    Ok(Reply::new(&s)?
        .blob(vec![[1.5f32, 2.0, n as f32]; 100_000])
        .blob(vec![7u8; 3]))
}

fn admit(body: &[u8], task: &mut Task) -> Result<Reply, Failure> {
    let peak: u64 = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    task.admit(peak)?;
    Reply::new(&peak)
}

fn fail(_: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    Err(Failure::Failed("bad file".into()))
}

fn big(body: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    let n: usize = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    Reply::new(&n).map(|r| r.blob(vec![[0.25f32; 3]; n]))
}

fn crash(_: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    std::process::abort()
}

fn panics(_: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    panic!("oops")
}

fn polls(_: &[u8], task: &mut Task) -> Result<Reply, Failure> {
    while !task.cancel_token().is_cancelled() {
        std::thread::sleep(Duration::from_millis(5));
    }
    Err(Failure::Cancelled)
}

#[allow(clippy::empty_loop)]
fn hangs(_: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() {
    if aigle_isolate::is_worker() {
        aigle_isolate::serve(&[
            ("echo", echo),
            ("big", big),
            ("admit", admit),
            ("fail", fail),
            ("crash", crash),
            ("panic", panics),
            ("poll", polls),
            ("hang", hangs),
        ]);
    }
    assert!(aigle_isolate::enable("/proc/self/exe".into(), Vec::new()));
    let tests: &[(&str, fn())] = &[
        ("blobs", blobs),
        ("admission", admission),
        ("errors", errors),
        ("crash_then_recover", crash_then_recover),
        ("cancel", cancel),
        ("cancel_hung", cancel_hung),
        ("parallel", parallel),
        ("large_result", large_result),
    ];
    for (name, t) in tests {
        eprint!("test {name} ... ");
        t();
        eprintln!("ok");
    }
}

fn blobs() {
    let mut c = Call::start("echo", &"hello", None).unwrap();
    let Event::Done(h) = c.event::<String>().unwrap() else {
        panic!("admit")
    };
    assert_eq!(h, "hello");
    assert_eq!(c.blobs_left(), 2);
    let a: Vec<[f32; 3]> = c.blob().unwrap();
    assert_eq!(a.len(), 100_000);
    assert_eq!(a[99_999], [1.5, 2.0, 5.0]);
    let b: Vec<u8> = c.blob().unwrap();
    assert_eq!(b, vec![7, 7, 7]);
}

fn admission() {
    let mut c = Call::start("admit", &123u64, None).unwrap();
    assert!(matches!(c.event::<u64>().unwrap(), Event::Admit(123)));
    c.admitted(true).unwrap();
    assert!(matches!(c.event::<u64>().unwrap(), Event::Done(123)));

    let mut c = Call::start("admit", &5u64, None).unwrap();
    assert!(matches!(c.event::<u64>().unwrap(), Event::Admit(5)));
    c.admitted(false).unwrap();
    assert_eq!(c.event::<u64>().err(), Some(Error::Cancelled));
}

fn errors() {
    assert_eq!(
        call::<()>("fail", &(), None),
        Err(Error::Failed("bad file".into()))
    );
    match call::<()>("panic", &(), None) {
        Err(Error::Failed(m)) => assert!(m.contains("oops"), "{m}"),
        r => panic!("{r:?}"),
    }
    assert!(matches!(
        call::<()>("nope", &(), None),
        Err(Error::Failed(_))
    ));
}

fn crash_then_recover() {
    match call::<()>("crash", &(), None) {
        Err(Error::Crashed(m)) => assert!(m.contains("SIGABRT"), "{m}"),
        r => panic!("{r:?}"),
    }
    assert_eq!(call::<u64>("admit", &9u64, None), Ok(9));
}

fn cancel() {
    let token = CancelToken::new();
    let t = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        t.cancel();
    });
    let t0 = Instant::now();
    assert_eq!(call::<()>("poll", &(), Some(&token)), Err(Error::Cancelled));
    assert!(t0.elapsed() < Duration::from_secs(1));
    // The worker survived and is reused.
    assert_eq!(call::<u64>("admit", &1u64, None), Ok(1));
}

fn cancel_hung() {
    let token = CancelToken::new();
    let t = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        t.cancel();
    });
    let t0 = Instant::now();
    assert_eq!(call::<()>("hang", &(), Some(&token)), Err(Error::Cancelled));
    assert!(t0.elapsed() < Duration::from_secs(4));
}

fn parallel() {
    let threads: Vec<_> = (0..12u64)
        .map(|i| std::thread::spawn(move || call::<u64>("admit", &i, None)))
        .collect();
    for (i, t) in threads.into_iter().enumerate() {
        assert_eq!(t.join().unwrap(), Ok(i as u64));
    }
}

/// A 24-megapixel f32 RGB image (288 MB).
fn large_result() {
    let n = 6000 * 4000;
    let t0 = Instant::now();
    let mut c = Call::start("big", &n, None).unwrap();
    assert!(matches!(c.event::<usize>().unwrap(), Event::Done(m) if m == n));
    let v: Vec<[f32; 3]> = c.blob().unwrap();
    assert_eq!(v.len(), n);
    assert_eq!(v[n - 1], [0.25; 3]);
    eprint!("({:?}) ", t0.elapsed());
}
