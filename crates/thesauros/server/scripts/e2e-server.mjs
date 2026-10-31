import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';
const directory = mkdtempSync(join(tmpdir(), 'thesauros-browser-'));
const config = join(directory, 'server.toml');
writeFileSync(config, `data = ${JSON.stringify(join(directory, 'data'))}\nlisten = "127.0.0.1:18787"\nmin_free_bytes = 0\n`);
const environment = { ...process.env, THESAUROS_TOKEN: 'thesauros-browser-test-token-32-characters', THESAUROS_LOG: 'warn' };
for (const name of ['THESAUROS_DATA', 'THESAUROS_LISTEN', 'THESAUROS_PUBLIC_URL', 'THESAUROS_TOKEN_FILE', 'AIGLE_ISOLATE']) delete environment[name];
const child = spawn(process.env.THESAUROS_TEST_BINARY || resolve('../../../target/debug/thesauros'), ['--config', config], {
  stdio: 'inherit',
  env: environment,
});
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => child.kill('SIGTERM'));
child.on('error', (error) => { console.error(error); process.exitCode = 1; });
child.on('exit', (code) => { rmSync(directory, { recursive: true, force: true }); process.exit(code || 0); });
