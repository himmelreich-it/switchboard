// P4 probe: the config options and slash commands claude-agent-acp reports, and whether a model switch takes effect
// on a live session. Run: bun spike/acp/config-probe.ts (from the repo root). Uses the app's own driver.
import { openSession } from "../../acp";

const updates: any[] = [];
const live = await openSession({ cwd: process.cwd(), mcpServers: [] }, {
  onUpdate: (u) => updates.push(u),
  onPermission: () => ({ outcome: "cancelled" }),
});
const ids = () => JSON.stringify(live.configOptions.map((o: any) => [o.id, o.currentValue]));
console.log("new", ids());
await Bun.sleep(1500);
const cmds = updates.filter((u) => u.sessionUpdate === "available_commands_update").at(-1)?.availableCommands ?? [];
console.log("commands", cmds.length, cmds.filter((c: any) => !c.name.includes(":")).map((c: any) => c.name).join(" "));
await live.setConfig("effort", "low");
console.log("effort low", ids());
await live.setConfig("model", "haiku");
console.log("haiku", ids());
updates.length = 0;
await live.prompt("Which Claude model are you? Answer with the model id only.");
console.log("answer", updates.filter((u) => u.sessionUpdate === "agent_message_chunk").map((u) => u.content?.text).join(""));
console.log("updates", [...new Set(updates.map((u) => u.sessionUpdate))], JSON.stringify(updates.filter((u) => u.sessionUpdate === "usage_update").at(-1)?._meta));
console.log("config updates", JSON.stringify(updates.filter((u) => u.sessionUpdate === "config_option_update").map((u) => u.configOptions?.map((o: any) => [o.id, o.currentValue]))));
await live.setConfig("model", "opus");
console.log("opus", ids());
updates.length = 0;
await live.prompt("Which Claude model are you? Answer with the model id only.");
console.log("answer", updates.filter((u) => u.sessionUpdate === "agent_message_chunk").map((u) => u.content?.text).join(""));
live.close();
process.exit(0);
