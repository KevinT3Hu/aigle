# Thesauros

Thesauros is Phaos's single-owner RAW library server. It uses **Topcoat 0.11.0**
for routing and Rust HTML views, **HTMX 2.0.11** for server-rendered updates, and
compiled **Tailwind CSS 4.3.3**. Browser assets are pinned and served by the
binary; running the service requires no Node installation or external CDN.

The server is in `crates/thesauros/server`. Shared RAW, Exiv2, XMP, isolation,
and client libraries are directly under `crates/`; it does not link Aigle's
renderer or desktop UI. Topcoat's Tower adapters mount the JSON API and
streaming Axum handlers under its router.

## Build and run locally

Use the workspace's pinned Rust toolchain. Native prerequisites are a C++
compiler and native build tools. Native libraries are pinned and built privately:

```sh
cargo xtask native-deps --server
```

See [native dependency setup](../../native/README.md) for host prerequisites.

```sh
cd ~/code/phaos
# Committed browser assets are sufficient for cargo builds.
cargo build --release --locked -p thesauros -p aigle-remote
export THESAUROS_TOKEN="$(openssl rand -hex 32)"
./target/release/thesauros --data ./thesauros-data --import-root /path/to/photos
```

Open `http://127.0.0.1:8787` and sign in with the token. The responsive web UI
supports batch upload, import, search, rating filters, previews, job progress,
cancel/retry, reindexing, and RAW downloads. Tokens must contain 32 through 1024 printable ASCII
characters. Keep the data directory separate from import roots.

Configuration loads in this order: built-in defaults, `--config FILE`, supported
environment overrides, then CLI flags. Relative paths are relative to the
process's working directory. Unknown config keys and invalid bounds fail startup.
Use absolute paths for a service installation.

```sh
./target/release/thesauros --config /etc/thesauros/server.toml --check-config
```

Supported environment overrides are `THESAUROS_DATA`, `THESAUROS_LISTEN`,
`THESAUROS_PUBLIC_URL`, `THESAUROS_TOKEN_FILE`, and `THESAUROS_LOG`. `THESAUROS_TOKEN`
is used only when no token file is configured. Token files must have no group
or other-user permissions (`chmod 600`). Do not put the token in TOML or command
arguments. CLI flags include `--config`, `--data`, repeatable `--import-root`,
`--listen`, `--public-url`, and `--token-file`.

## Production deployment

Examples are in [deploy/](deploy/): a bounded production configuration, an
unprivileged systemd service, and a Caddy HTTPS reverse proxy. These are deployment
artifacts, not an installation performed by the development agent.

1. Build with `--release --locked` and install the server as
   `/usr/local/bin/thesauros`. Its image libraries are linked from the pinned vcpkg
   build. Its C++ runtime and OpenMP libraries must also be installed on the host.
2. Create a `thesauros` system user and `/etc/thesauros`. Generate a random token
   with `openssl rand -hex 32`, store it in `/etc/thesauros/token` owned by root,
   and set mode `600`. systemd's `LoadCredential` gives the service a private
   readable copy; the config template intentionally contains no secret.
3. Copy `server.toml` and `thesauros.service` into their system locations. Set
   `public_url` to your exact HTTPS origin. Ensure `/srv/photos` exists and give
   the service account only the required read access and sidecar write access.
   The example enables these imported sidecar writes with `ReadWritePaths`.
4. Configure Caddy for the same domain, point DNS to the host, and expose only
   the HTTPS proxy. Keep the upstream listener on loopback. Preserve the
   external Host header; forwarded headers are not trusted for authentication.
5. Run `systemctl daemon-reload`, enable/start `thesauros`, and verify
   `/health/ready` through the proxy. Use `journalctl -u thesauros` for JSON logs.

A non-loopback listener requires `public_url`; HTTPS is required there except
for localhost development origins. HTTPS enables Secure `__Host-` cookies and
HSTS. Browser sessions are HttpOnly, SameSite=Strict, expire after 12 hours by
default, and store only token hashes in SQLite. Credential rotation invalidates
old sessions. Browser mutations require both an exact Origin match and a
session-bound CSRF token. API bearer tokens are separate from cookie auth;
browsers cannot use session cookies to call the management API. CSP disables
inline scripts/eval, and HTMX history caching and script execution are disabled.
The service applies a global 20-attempt/minute authentication-failure limit;
add per-client rate limits at the public proxy if needed.

The token grants full owner access. This deployment model has one owner and
trusted import directories; it is not a multi-tenant service with user roles,
OIDC, or quotas per user. Topcoat is an early-stage framework, so its version is
pinned and upgrades require rerunning the browser and integration suites.

## Storage, jobs, and recovery

Uploads stream to same-filesystem temporary files and compute BLAKE3 IDs while
receiving bytes. Successful transfers return a catalog entry immediately with
`status: "pending"`; bounded background workers extract metadata and previews.
Identical content deduplicates without overwriting an original. Defaults allow
four concurrent uploads, 4 GiB per file, up to 100 files per browser batch, and
roughly 16 GiB per multipart request. Requests exceeding configured limits are
rejected. Reservations preserve the configured free-space margin; partial
uploads are removed when their requests fail and stale upload temps are cleaned
on restart. The upload request deadline defaults to 15 minutes. An admission limit of 128
requests is held through response streaming; health checks remain available
under saturation. Tokio worker and blocking-thread counts are also bounded. Retrying a
partially completed batch is safe because completed files deduplicate.

The SQLite catalog uses WAL, FULL synchronous commits, foreign keys, busy
timeouts, indexes, transactional migrations, and short-lived connections.
Browsing reads cached metadata and never invokes native parsers. No global
catalog mutex is held during parsing or directory traversal. A data-directory
file lock permits exactly one server instance per catalog.

Import requests return `202` with a persisted job. Imports traverse folders with
bounded depth and memory and commit progress after each file. Resolved paths
must be in configured roots; directory symlinks are skipped and RAW file
symlinks encountered during traversal are rejected. Imported RAWs stay in place;
edits are written next to them using Aigle's sidecar preference:
`photo.NEF.xmp`, then `photo.xmp`. Symbolic-link sidecars cannot be downloaded or
overwritten through the API. Treat imported RAWs as immutable: changed size or
mtime produces a `409` download conflict. Move modified replacements to a new
path before importing them; old records preserve the prior content identity.
External sidecar changes require explicit reindexing to refresh cached metadata.

Jobs are persisted before execution, claimed transactionally, and reclaimed
following a restart. Three interrupted attempts mark a job failed; the owner
can retry it explicitly. Cancellation takes effect between files; an active
native parse may finish first. SIGINT/SIGTERM stop new work, interrupt imports
between files, drain in-flight work up to the configured deadline, and leave
unfinished jobs resumable. Failed and interrupted files are visible in Activity.
The API returns up to 50 jobs, prioritizing running and queued work ahead of
recent history; the UI shows 20.
Completed/cancelled job history is pruned after 30 days, sessions on expiry.

Native RAW/EXIF and received XMP parsing run in isolated processes. All parser
calls share a concurrency budget, parser address space is bounded (2 GiB by
default), and image decoding has allocation/dimension limits. Disabling parser
isolation through `AIGLE_ISOLATE=0` is rejected at startup. Workers run as the
service account; systemd hardening and filesystem permissions define their
filesystem access. RAWs with no embedded
preview show a placeholder; invalid RAWs retain their original bytes but are
marked failed rather than presented as successfully indexed.

Sidecar writes require `If-Match`, validate XMP/rating in a worker, write a
FULL-synchronous SQLite journal, replace the file atomically with file/directory
fsync, and update cached metadata transactionally. A crash between these steps
is replayed at startup. Recovery stops if an external edit conflicts with a
journal entry, preserving both versions for operator reconciliation. Originals
persisted just before an interrupted registration are recovered by content ID
and receive a `Recovered-…` display name because the original upload name may
not have been committed. Do not edit imported sidecars concurrently with server
writes: filesystem writers outside the service cannot participate in its
transaction; revision checks detect prior external changes, not arbitrary
uncooperative writes at the exact instant of rename.

## Editing with Aigle

```sh
# Set THESAUROS_TOKEN in the client environment.
cargo run -p aigle-remote -- list https://photos.example.com
cargo run -p aigle-app -- --remote https://photos.example.com PHOTO_ID
# Or explicitly check out and launch a desktop binary:
cargo run -p aigle-remote -- checkout https://photos.example.com PHOTO_ID \
  --cache ./checkouts --open ./target/debug/aigle-gui
# After saving in Aigle, push the checkout's XMP:
cargo run -p aigle-remote -- push ./checkouts/PHOTO_ID
cargo run -p aigle-remote -- upload https://photos.example.com /photos/*.NEF
cargo run -p aigle-remote -- import https://photos.example.com /srv/photos
```

Aigle's persistent remote checkout path appears in its log. Existing local edits
are preserved on repeated opens. The client verifies the RAW content hash and
synchronizes the full XMP packet. Tokens are never stored in the checkout.
A `412` response preserves local edits: download the server XMP, reconcile both,
and update the checkout's base revision before pushing again. Sync remains an
explicit push; this change does not add a remote library browser to Aigle.

## API

All `/api/` endpoints require `Authorization: Bearer TOKEN`. HTTP errors use
`{ "error": "safe message", "code": "machine_code" }`; framework-level malformed
body/query and size-limit rejections may be plain text. Internal failures are
logged with a request ID and returned as redacted 500 responses. Filenames and
metadata in HTML use Topcoat's context-sensitive escaping.

| Method | Path | Response / purpose |
| --- | --- | --- |
| GET | `/api/photos?q=&rating=&camera=&lens=&limit=60&offset=0` | Search, exact camera/lens/rating filters, pagination (limit <= 500) |
| GET | `/api/photos/{id}` | Cached metadata, rating, status, preview flag, and XMP revision |
| POST | `/api/upload` | RAW body, Content-Length, percent-encoded X-Filename; 201 pending photo |
| POST | `/api/import` | JSON `{ "path": "/srv/photos" }`; 202 `{ "job": … }` |
| POST | `/api/photos/{id}/reindex` | 202 job, coalesced with an active job for the photo |
| GET | `/api/jobs` | Up to 50 jobs, active first, with progress/failure counts |
| GET | `/api/jobs/{id}` | Poll job state and progress |
| POST | `/api/jobs/{id}/cancel` | Cancel queued work or request running cancellation |
| POST | `/api/jobs/{id}/retry` | Requeue a failed/cancelled job |
| GET/HEAD | `/api/photos/{id}/raw` | Stream original; Range/If-Range/ETag support |
| GET/HEAD | `/api/photos/{id}/preview` | Oriented JPEG, max edge 1600px; 404 if unavailable |
| GET | `/api/photos/{id}/sidecar` | XMP and quoted ETag; 404/`"none"` if absent |
| PUT | `/api/photos/{id}/sidecar` | XMP <= 8 MiB, required If-Match; 412 conflict |
| GET | `/health/live` | Unauthenticated process liveness |
| GET | `/health/ready` | Unauthenticated database and free-space readiness |

Native previews are embedded RAW thumbnails, not a render of XMP edits.
Downloads support resumption at the HTTP layer; upload chunks/resumable upload
sessions are not implemented. There is no automatic filesystem watcher or
original-deletion endpoint.

## Backup, restore, and upgrades

Back up both the catalog **and** original/XMP files. Stop the service, snapshot
or copy the complete data directory (including any SQLite WAL/SHM files), and
snapshot configured import roots with their sidecars. Restart after the
consistent copy. Never delete journal entries or originals as a substitute for
backup. Test a restore into separate paths with the service stopped; then start
one instance and verify readiness, catalog counts, RAW downloads, and XMP.

Before upgrading, take this backup and stop the old binary. Startup applies
migrations transactionally and refuses a schema from a newer server version.
Prototype catalogs are adopted and existing photos are reindexed gradually by
the job workers to populate the new metadata cache.
Rolling back a schema-changing release requires restoring its matching backup;
replacing only the binary is insufficient. To rotate credentials, replace the
protected token file and restart the service. Existing browser sessions then
fail validation. Inspect JSON logs for job failures, request IDs, and recovery
notices. A readiness 503 can indicate storage pressure or database failure;
create free space and inspect logs before restarting repeatedly.

## Development and validation

```sh
cargo test -p thesauros -p aigle-remote
cargo clippy -p thesauros --all-targets --no-deps -- -D warnings
cd crates/thesauros/server
npm ci --ignore-scripts
npm run build                     # Update the committed, local production assets.
# Rust embeds the built assets, so rebuild the binary after changing them:
cargo build -p thesauros
npx playwright install chromium   # Once, or provide CHROME_BIN for installed Chrome.
npm run test:browser
```

Rust integration tests launch the real server and native workers using an
invented DNG fixture (no real photograph). They cover metadata/preview
extraction, range/HEAD downloads, deduplication, Aigle checkout/sync, conflict
checks, auth/CSRF, XSS escaping, limits, error redaction, and health. Storage
tests exercise old-catalog migration, process locking, capacity, restart
recovery, and journal replay/conflict preservation. Playwright covers sign-in,
CSP/local assets, actual HTMX upload/search/rating/download behavior, and phone
layout. The Thesauros GitHub Actions workflow runs these checks for relevant changes.
Neither the service units nor a public HTTPS deployment are installed
or exercised automatically by these tests.
