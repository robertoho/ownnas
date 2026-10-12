// Installed by OwnNAS (Settings → Terminal → Agent integrations), which also removes it.
// Reports what opencode is doing to the OwnNAS terminal it runs in, so the OwnNAS page can
// show its status and notify you. Outside OwnNAS terminals it does nothing.
const URL = process.env.OWNNAS_AGENT_URL;
const TOKEN = process.env.OWNNAS_AGENT_TOKEN;

const FORWARD = new Set([
  "session.created",
  "session.updated",
  "session.status",
  "session.idle",
  "session.error",
  "permission.asked",
  "permission.updated",
  "permission.replied",
  "question.asked",
  "question.replied",
  "file.edited",
]);

// Subagent sessions must not make the main session look idle or finished.
const childSessions = new Set();
const toolArgs = new Map();
let queue = Promise.resolve();

function send(event) {
  queue = queue
    .then(() =>
      fetch(URL, {
        method: "POST",
        headers: { "content-type": "application/json", "x-ownnas-agent-token": TOKEN },
        body: JSON.stringify({ agent: "opencode", event }),
        signal: AbortSignal.timeout(2000),
      }),
    )
    .catch(() => {});
}

function pickArgs(args) {
  if (!args || typeof args !== "object") return {};
  const text = (value, max) => (typeof value === "string" ? value.slice(0, max) : undefined);
  return {
    filePath: text(args.filePath, 1000),
    command: text(args.command, 300),
    pattern: text(args.pattern, 200),
    url: text(args.url, 300),
    description: text(args.description, 200),
  };
}

function pickProperties(properties) {
  const info = properties.info;
  return {
    sessionID: properties.sessionID,
    status: properties.status,
    error: properties.error,
    file: properties.file,
    title: properties.title,
    permission: properties.permission,
    type: properties.type,
    info: info ? { id: info.id, title: info.title } : undefined,
  };
}

export const OwnNASAgentPlugin = async () => {
  if (!URL || !TOKEN) return {};
  send({ type: "plugin.loaded" });
  return {
    "chat.message": async (input) => {
      if (!childSessions.has(input?.sessionID)) send({ type: "chat.message", properties: { sessionID: input?.sessionID } });
    },
    "tool.execute.before": async (input, output) => {
      if (childSessions.has(input?.sessionID)) return;
      const args = pickArgs(output?.args);
      if (input?.callID) toolArgs.set(input.callID, args);
      send({ type: "tool.execute.before", tool: input?.tool, args });
    },
    "tool.execute.after": async (input) => {
      const args = toolArgs.get(input?.callID) ?? pickArgs(input?.args);
      toolArgs.delete(input?.callID);
      if (!childSessions.has(input?.sessionID)) send({ type: "tool.execute.after", tool: input?.tool, args });
    },
    event: async ({ event }) => {
      const type = event?.type;
      const properties = event?.properties ?? {};
      const info = properties.info;
      if (info?.id && info.parentID) childSessions.add(info.id);
      if (!FORWARD.has(type)) return;
      const sessionID = properties.sessionID ?? info?.id;
      const asksUser = type.startsWith("permission.") || type.startsWith("question.");
      if (sessionID && childSessions.has(sessionID) && !asksUser) return;
      send({ type, properties: pickProperties(properties) });
    },
  };
};
