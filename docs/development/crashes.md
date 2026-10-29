# Bad files, logs and crash reports

## Worker processes

LibRaw, librtprocess, libvips and Exiv2 parse photo files in C and C++. In the app, file
decoding (`engine.decode`), embedded raw previews (`engine.preview`), EXIF facts
(`library.facts`) and embedded XMP (`library.xmp`) run in worker processes: the app's own
binary started as `aigle-gui --aigle-worker` (`crates/isolate`). If a worker crashes, aborts
or is silent for 5 minutes, it is killed and replaced, and only that file fails. The grid and
filmstrip show it as "Unreadable", with the reason in a tooltip; develop shows "Cannot open".

Decoded pixels come back over a pipe, so a decode costs one extra copy (about 0.3 s for a
24-megapixel image). Sidecars and XMP packets written by aigle are still parsed in-process.

`AIGLE_ISOLATE=0` parses in-process, which is easier to debug. Tests, examples and other
programs using the engine or library parse in-process unless they call
`aigle_isolate::enable`.

## Logs

Each run logs to stderr and to `$XDG_STATE_HOME/aigle/logs/aigle-<UTC time>-<pid>.log`
(`~/.local/state/aigle/logs`). The newest 10 are kept. Workers append to their app's log.
`RUST_LOG` sets the level (default `warn,aigle=info`).

## Crash reports

Reports go to `$XDG_STATE_HOME/aigle/crashes/` (newest 20 kept), with the version, kernel,
desktop and the last 200 lines of the log:

* A Rust panic writes one right away, with a backtrace (at most 3 per run).
* A run that ends without a clean exit or SIGTERM/SIGINT/SIGHUP (a native crash, or the
  out-of-memory killer) gets one at the next start, from `sessions/<pid>`. That start also
  shows a notification with the report's path.
