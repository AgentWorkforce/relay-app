// AGENT_RELAY_HOSTED_PI_MCP_V1
// Agent Relay's hosted MCP registration. Render the config path as a JSON string.
// This is a Pi extension, not an MCP server or a local transport.
import { constants, closeSync, fstatSync, openSync, readFileSync } from 'node:fs';

const configPath = __RELAY_CONFIG_PATH__;
const serverName = 'agent-relay';
const identityHeader = 'X-Agent-Relay-Native-Session-Id';

function configuration() {
  const fd = openSync(configPath, constants.O_RDONLY | (constants.O_NOFOLLOW || 0));
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > 65536 ||
        (typeof process.getuid === 'function' &&
         (stat.uid !== process.getuid() || (stat.mode & 0o077)))) {
      throw new Error('Invalid Agent Relay provider configuration');
    }
    const config = JSON.parse(readFileSync(fd, 'utf8'));
    const url = new URL(config.url);
    if ((url.protocol !== 'https:' && !(url.protocol === 'http:' &&
         ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) ||
        url.username || url.password || url.search || url.hash ||
        url.pathname !== '/cloud/api/v1/mcp/shared-sessions' ||
        !config.headers || typeof config.headers !== 'object' || Array.isArray(config.headers)) {
      throw new Error('Invalid Agent Relay provider configuration');
    }
    const headers = {};
    for (const [name, value] of Object.entries(config.headers)) {
      if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name) ||
          typeof value !== 'string' || /[\r\n]/.test(value)) {
        throw new Error('Invalid Agent Relay provider configuration');
      }
      // Only Pi's runtime may provide the current session's identity.
      if (name.toLowerCase() !== identityHeader.toLowerCase()) headers[name] = value;
    }
    if (!Object.entries(headers).some(([key, value]) =>
      key.toLowerCase() === 'authorization' && /^Bearer \S+$/.test(value))) {
      throw new Error('Agent Relay is not connected');
    }
    return { url: url.href, headers };
  } finally {
    closeSync(fd);
  }
}

export default function agentRelayMcp(pi) {
  if (typeof pi.registerMcpServer !== 'function' || typeof pi.unregisterMcpServer !== 'function') {
    throw new Error('Update Pi to enable Agent Relay MCP');
  }
  let registered;
  const clear = () => {
    if (registered) pi.unregisterMcpServer(serverName);
    registered = undefined;
  };
  const synchronize = (_event, ctx) => {
    try {
      const sessionId = ctx.sessionManager.getSessionId();
      if (typeof sessionId !== 'string' ||
          !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(sessionId)) {
        throw new Error('Agent Relay could not identify this Pi session');
      }
      const config = configuration();
      config.headers[identityHeader] = sessionId;
      config.exposure = 'direct';
      const next = JSON.stringify(config);
      if (next !== registered) {
        pi.registerMcpServer(serverName, config);
        registered = next;
      }
    } catch {
      clear();
      // Never include parse errors, URLs, or header values in an agent transcript.
      throw new Error('Agent Relay is unavailable. Check its connection in the Agent Relay app.');
    }
  };
  pi.on('session_start', synchronize);
  pi.on('before_agent_start', synchronize);
  pi.on('session_shutdown', clear);
}
