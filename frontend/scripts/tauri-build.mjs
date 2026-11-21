import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontend = fileURLToPath(new URL('../', import.meta.url));
const args = process.argv.slice(2);
const runnerArgs = args.indexOf('--');
const options = runnerArgs < 0 ? args : args.slice(0, runnerArgs);
const rustFeature = 'rust-qualification';

function selectedBackend() {
  const mode = options.some((arg) => arg === '--debug' || arg === '-d')
    ? 'development'
    : 'production';
  const directory = mkdtempSync(join(tmpdir(), 'matterdraft-backend-'));
  try {
    const program = join(directory, process.platform === 'win32' ? 'selector.exe' : 'selector');
    const source = fileURLToPath(new URL('./read-backend-selection.rs', import.meta.url));
    const compile = spawnSync(
      process.env.RUSTC || 'rustc',
      ['--edition=2021', source, '-o', program],
      {
        stdio: 'inherit',
      },
    );
    if (compile.status !== 0)
      throw compile.error || new Error('Cannot compile the backend selector.');
    const selection = spawnSync(program, [frontend, mode], { encoding: 'utf8' });
    if (selection.status !== 0) {
      throw (
        selection.error ||
        new Error(selection.stderr.trim() || 'Cannot read the backend selection.')
      );
    }
    return selection.stdout.trim();
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function rustPackagingOptions() {
  const defaultConfig = ['--config', 'src-tauri/tauri.rust-ocr.conf.json'];
  const hasConfig = options.some(
    (arg) =>
      arg === '--config' || arg === '-c' || arg.startsWith('--config=') || arg.startsWith('-c='),
  );
  const explicitQualificationConfig = hasConfig ? [] : defaultConfig;
  const featureOption = options.findIndex(
    (arg) =>
      arg === '-f' ||
      arg === '--features' ||
      arg.startsWith('--features=') ||
      arg.startsWith('-f='),
  );
  if (featureOption >= 0) {
    const flag = options[featureOption];
    const inline = flag.indexOf('=');
    if (inline >= 0) {
      if (
        flag
          .slice(inline + 1)
          .split(',')
          .includes(rustFeature)
      )
        return explicitQualificationConfig;
      args[featureOption] = flag + ',' + rustFeature;
    } else {
      let index = featureOption + 1;
      while (index < options.length && !options[index].startsWith('-')) {
        if (options[index].split(',').includes(rustFeature)) return explicitQualificationConfig;
        index += 1;
      }
      args.splice(featureOption + 1, 0, rustFeature);
    }
  } else {
    args.unshift(`--features=${rustFeature}`);
  }
  return defaultConfig;
}

try {
  const informational = options.some((arg) => ['--help', '-h', '--version', '-V'].includes(arg));
  const backend = informational ? '' : selectedBackend();
  const packaging = backend === 'rust' ? rustPackagingOptions() : [];
  const tauri = fileURLToPath(new URL('../node_modules/@tauri-apps/cli/tauri.js', import.meta.url));
  const result = spawnSync(process.execPath, [tauri, 'build', ...packaging, ...args], {
    cwd: frontend,
    env: backend ? { ...process.env, MATTERDRAFT_BACKEND: backend } : process.env,
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} catch (error) {
  process.stderr.write(`MatterDraft package build: ${error.message}\n`);
  process.exitCode = 1;
}
