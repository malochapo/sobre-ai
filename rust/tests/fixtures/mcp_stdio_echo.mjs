#!/usr/bin/env node
// Minimal MCP stdio server fixture for the sobre gateway E2E test (#1077).
//
// Speaks newline-delimited JSON-RPC 2.0 over stdin/stdout — the MCP stdio wire
// protocol — implementing just enough to exercise the gateway's real spawn path:
//   initialize  -> handshake (echoes the client's protocolVersion)
//   tools/list  -> advertises `echo` and `boom`
//   tools/call  -> `echo` returns `echo:<text>`; `boom` exits without replying
//                  (simulates a child dying mid-call, for the pool self-heal test)
// No dependencies, no network; only Node.js is required.

import { createInterface } from 'node:readline';

const rl = createInterface({ input: process.stdin });

function send(msg) {
  process.stdout.write(JSON.stringify(msg) + '\n');
}

rl.on('line', (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;

  let req;
  try {
    req = JSON.parse(trimmed);
  } catch {
    return;
  }

  const { id, method, params } = req;

  if (method === 'initialize') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: (params && params.protocolVersion) || '2025-06-18',
        capabilities: { tools: {} },
        serverInfo: { name: 'mcp-stdio-echo', version: '0.1.0' },
      },
    });
  } else if (method === 'notifications/initialized') {
    // Notification: no response.
  } else if (method === 'tools/list') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        tools: [
          {
            name: 'echo',
            description: 'Echo back the provided text',
            inputSchema: {
              type: 'object',
              properties: { text: { type: 'string' } },
              required: ['text'],
            },
          },
          {
            name: 'boom',
            description: 'Exit the process without replying (test fault injection)',
            inputSchema: { type: 'object', properties: {} },
          },
        ],
      },
    });
  } else if (method === 'tools/call') {
    const name = params && params.name;
    const args = (params && params.arguments) || {};
    if (name === 'boom') {
      // Die mid-request, never sending a response: the client's transport sees
      // EOF on a pending call. Exercises pool eviction + reopen (no blind retry).
      process.exit(1);
    } else if (name === 'echo') {
      send({
        jsonrpc: '2.0',
        id,
        result: {
          content: [{ type: 'text', text: 'echo:' + (args.text ?? '') }],
          isError: false,
        },
      });
    } else {
      send({
        jsonrpc: '2.0',
        id,
        error: { code: -32602, message: 'unknown tool: ' + name },
      });
    }
  } else if (id !== undefined && id !== null) {
    // Any other request gets a method-not-found so the client never hangs.
    send({
      jsonrpc: '2.0',
      id,
      error: { code: -32601, message: 'method not found: ' + method },
    });
  }
});

// When the client closes stdin (session dropped from the pool), exit cleanly.
rl.on('close', () => process.exit(0));
