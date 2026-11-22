// Host qualification only. Read the extracted service's native settings wire;
// do not retain settings bodies, connection details or CLI authentication output.
import { connect } from "node:net";
import { setTimeout as delay } from "node:timers/promises";

const [path, model, effort] = process.argv.slice(2);
const engine = "codex-acp";
const maxFrame = 8 * 1024 * 1024; // workspace_protocol::FRAME_BYTES
let nextId = 0;
class GateFailure extends Error {}
const require = (condition, reason) => { if (!condition) throw new GateFailure(reason); };

async function machine(op, params = {}) {
  const id = ++nextId;
  const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method: "machine.call", params: { op, params } }));
  const header = Buffer.alloc(4);
  header.writeUInt32BE(body.length);
  const socket = connect(path);
  const timer = setTimeout(() => socket.destroy(), 10_000);
  try {
    socket.on("connect", () => socket.write(Buffer.concat([header, body])));
    let frame = Buffer.alloc(0);
    for await (const chunk of socket) {
      frame = Buffer.concat([frame, chunk]);
      if (frame.length < 4) continue;
      const size = frame.readUInt32BE();
      require(size > 0 && size <= maxFrame && frame.length <= size + 4, "invalid_settings_frame");
      if (frame.length !== size + 4) continue;
      const reply = JSON.parse(frame.subarray(4).toString("utf8"));
      require(reply.jsonrpc === "2.0" && reply.id === id && !reply.error, "invalid_settings_rpc_reply");
      const result = reply.result;
      const safeCode = typeof result?.code === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(result.code)
        ? result.code : "machine_operation_refused";
      require(result?.ok === true && result.result, safeCode);
      return result.result;
    }
    throw new GateFailure("incomplete_settings_frame");
  } finally {
    clearTimeout(timer);
    socket.destroy();
  }
}

try {
  require(!Object.hasOwn(process.env, "CORTEX_ROUTING"), "inherited_routing_override");
  const initial = await machine("config.read");
  require(initial.config && !initial.config.unavailable && initial.config.files?.invalidRouting === false,
    "machine_settings_unavailable");
  console.log("machine config.read=ok");
  // The service verifies its bundled native Codex binary's login-status exit
  // and registers only this run's connection. Catalogue discovery sends no prompt.
  await machine("engine.connection", { action: "recheck", engine });
  console.log("native codex login status exit=0 (service recheck)");

  const deadline = Date.now() + 65_000;
  for (;;) {
    const { config, catalogueRefresh } = await machine("config.read");
    require(config && !config.unavailable && config.files?.invalidRouting === false, "machine_settings_unavailable");
    const { routing, user_defaults: defaults } = config.surfaces ?? {};
    const entries = Object.entries(routing?.engines ?? {});
    const enabled = entries.flatMap(([kind, rules]) => rules.enabled.map(id => [kind, id]));
    require(enabled.length === 1 && enabled[0][0] === engine && enabled[0][1] === model, "unexpected_enabled_models");
    require(defaults?.routeDefault === null && defaults.coordinator?.engine === engine
      && defaults.coordinator.model === model && defaults.coordinator.parameters?.thought_level === effort,
    "unexpected_coordinator_default");

    const catalog = config.catalogue?.[engine];
    const matches = catalog?.models?.filter(row => row.id === model) ?? [];
    const row = matches[0];
    if (catalogueRefresh?.running === false && catalogueRefresh.failed?.length === 0
      && config.setup?.[engine]?.status === "connected" && catalog?.failed === false
      && Number.isFinite(catalog.observedAt) && catalog.observedAt > 0
      && matches.length === 1 && row.observed === true && row.efforts.includes(effort)
      && /^xr_[0-9a-f]{24}$/.test(row.routeId)
      && !routing.deny.some(token => [model, row.routeId, engine + "/" + model, engine + "/*"].includes(token))) {
      console.log("effective catalogue=observed route=" + engine + "/" + model + " id=" + row.routeId);
      console.log("portable coordinator default=matches effort=" + effort);
      break;
    }
    require(Date.now() < deadline, "effective_catalogue_or_default_unverified");
    await delay(1_000);
  }
} catch (error) {
  // Never print diagnostics containing a socket response or credentials.
  console.error("settings readiness refused: " + (error instanceof GateFailure ? error.message : "settings_probe_failed"));
  process.exitCode = 1;
}
