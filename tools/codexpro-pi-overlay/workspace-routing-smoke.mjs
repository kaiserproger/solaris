import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

function encode(message) {
  return `${JSON.stringify(message)}\n`;
}

class McpStdioClient {
  constructor(command, args, options) {
    this.child = spawn(command, args, options);
    this.buffer = '';
    this.nextId = 1;
    this.pending = new Map();
    this.child.stdout.on('data', (chunk) => this.onData(String(chunk)));
    this.child.stderr.on('data', (chunk) => process.stderr.write(chunk));
    this.child.on('exit', (code) => {
      for (const { reject } of this.pending.values()) reject(new Error(`server exited ${code}`));
    });
  }

  onData(chunk) {
    this.buffer += chunk;
    while (true) {
      const index = this.buffer.indexOf('\n');
      if (index < 0) return;
      const line = this.buffer.slice(0, index).replace(/\r$/, '');
      this.buffer = this.buffer.slice(index + 1);
      if (!line.trim()) continue;
      const msg = JSON.parse(line);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject, timer } = this.pending.get(msg.id);
        clearTimeout(timer);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      }
    }
  }

  request(method, params) {
    const id = this.nextId++;
    this.child.stdin.write(encode({ jsonrpc: '2.0', id, method, params }));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`timeout waiting for ${method}`)), 15000);
      timer.unref();
      this.pending.set(id, { resolve, reject, timer });
    });
  }

  notify(method, params = {}) {
    this.child.stdin.write(encode({ jsonrpc: '2.0', method, params }));
  }

  close() {
    this.child.kill('SIGTERM');
  }
}

function textOf(result) {
  return result.content?.find?.((part) => part.type === 'text')?.text ?? JSON.stringify(result.structuredContent ?? {});
}

async function expectToolError(client, name, args, pattern) {
  const result = await client.request('tools/call', { name, arguments: args });
  if (!result.isError) throw new Error(`${name} unexpectedly succeeded`);
  const text = textOf(result);
  if (!pattern.test(text)) throw new Error(`${name} error did not match ${pattern}: ${text}`);
  return text;
}

const codexproRoot = path.resolve(process.argv[2] ?? '');
if (!codexproRoot) throw new Error('CodexPro root argument is required');

const launchRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'codexpro-routing-launch-'));
const repoParent = await fs.mkdtemp(path.join(os.tmpdir(), 'codexpro-routing-parent-'));
const nestedRepo = path.join(repoParent, 'delivery-service');
await fs.mkdir(nestedRepo, { recursive: true });

const client = new McpStdioClient(
  process.execPath,
  ['dist/stdio.js', '--root', launchRoot, '--allow-root', launchRoot, '--allow-root', repoParent, '--bash', 'off', '--tool-mode', 'full'],
  {
    cwd: codexproRoot,
    env: {
      ...process.env,
      CODEXPRO_ROOT: launchRoot,
      CODEXPRO_ALLOWED_ROOTS: [launchRoot, repoParent].join(path.delimiter),
      CODEXPRO_TOOL_CARDS: '0'
    }
  }
);

try {
  await client.request('initialize', {
    protocolVersion: '2024-11-05',
    capabilities: {},
    clientInfo: { name: 'codexpro-workspace-routing-smoke', version: '0.1.0' }
  });
  client.notify('notifications/initialized');

  const tools = await client.request('tools/list', {});
  const byName = new Map(tools.tools.map((tool) => [tool.name, tool]));
  for (const name of ['spawn_pi_detached', 'spawn_pi_qa_detached', 'wait_pi_agent']) {
    const tool = byName.get(name);
    if (!tool) throw new Error(`missing Pi tool: ${name}`);
    const properties = tool.inputSchema?.properties ?? {};
    if (!properties.repo_root || !properties.repo_subdir) {
      throw new Error(`${name} does not expose repo_root/repo_subdir: ${JSON.stringify(tool.inputSchema)}`);
    }
  }

  const nestedError = await expectToolError(
    client,
    'wait_pi_agent',
    { repo_root: repoParent, repo_subdir: 'delivery-service', run_id: 'missing-run' },
    /Pi run metadata not found/
  );
  if (/Unknown workspace_id/.test(nestedError)) {
    throw new Error(`nested repo routing regressed to workspace-id lookup: ${nestedError}`);
  }

  await expectToolError(
    client,
    'wait_pi_agent',
    { repo_root: repoParent, repo_subdir: '../escape', run_id: 'missing-run' },
    /repo_subdir must be a relative path that stays inside repo_root/
  );

  const opened = await client.request('tools/call', {
    name: 'open_current_workspace',
    arguments: { include_tree: false }
  });
  await expectToolError(
    client,
    'wait_pi_agent',
    { workspace_id: opened.structuredContent.workspace_id, repo_root: repoParent, run_id: 'missing-run' },
    /accept workspace_id or repo_root, not both/
  );

  console.log('CodexPro workspace-routing smoke passed: Pi tools accept repo_root/repo_subdir without cross-request workspace state.');
} finally {
  client.close();
  await fs.rm(launchRoot, { recursive: true, force: true });
  await fs.rm(repoParent, { recursive: true, force: true });
}
