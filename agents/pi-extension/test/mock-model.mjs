// A scripted OpenAI-compatible chat model for tests (no real model is ever called). Prints {"port":n} on start.
// "[读取]" in the last user message: pv_search for 预算, then a short answer. "[慢]": answers after 5 s.
// "[引用]": pv_search for 预算, then a Markdown answer citing the first hit as [[event_id]].
// "运行技能「日回顾」": pv_search, then pv_save_draft of today's day review citing the first hit, then a short answer.
// "[改提醒:<item_id>]": pv_propose update_reminder for that item. "[提醒]": pv_propose a synthetic create_reminder. "[备注:<event_id>]": pv_annotate that event's notes. Both then answer.
import { createServer } from "node:http";
const textOf = (m) => (typeof m.content === "string" ? m.content : (m.content ?? []).map((p) => p.text ?? "").join(""));
const server = createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  const messages = body.messages ?? [];
  let u = messages.length - 1;
  while (u >= 0 && messages[u].role !== "user") u--;
  const fullUser = u >= 0 ? textOf(messages[u]) : "";
  const user = fullUser.split("用户当前请求：\n").at(-1);
  const tools = messages.slice(u + 1).filter((m) => m.role === "tool").length;
  let message = { role: "assistant", content: "好的。" };
  if (user.includes("[读取]") && tools === 0) message = { role: "assistant", content: null, tool_calls: [{ id: "call_1", type: "function", function: { name: "pv_search", arguments: JSON.stringify({ q: "预算" }) } }] };
  else if (user.includes("[读取]")) message = { role: "assistant", content: "找到了预算记录。" };
  else if (user.includes("运行技能「日回顾」")) {
    const toolText = messages.slice(u + 1).filter((m) => m.role === "tool").map(textOf).join("");
    const id = (toolText.match(/"event_id":"([^"]+)"/) ?? [])[1] ?? "none";
    const day = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    const today = new Date(), tomorrow = new Date(Date.now() + 86_400_000);
    if (tools === 0) message = { role: "assistant", content: null, tool_calls: [{ id: "call_d1", type: "function", function: { name: "pv_search", arguments: JSON.stringify({ q: "预算" }) } }] };
    else if (tools === 1) message = { role: "assistant", content: null, tool_calls: [{ id: "call_d2", type: "function", function: { name: "pv_save_draft", arguments: JSON.stringify({ level: "day", start: day(today), end: day(tomorrow), text: `【合成回顾】今天复核了季度预算 [[${id}]]。`, citations: [id] }) } }] };
    else message = { role: "assistant", content: `已保存今天的回顾草稿：今天复核了季度预算 [[${id}]]。` };
  }
  else if (user.includes("[引用]") && tools === 0) message = { role: "assistant", content: null, tool_calls: [{ id: "call_r", type: "function", function: { name: "pv_search", arguments: JSON.stringify({ q: "预算" }) } }] };
  else if (user.includes("[引用]")) {
    const toolText = messages.slice(u + 1).filter((m) => m.role === "tool").map(textOf).join("");
    const id = (toolText.match(/"event_id":"([^"]+)"/) ?? [])[1] ?? "none";
    message = { role: "assistant", content: `**季度预算**需要复核：\n\n- 见 [[${id}]]\n- 用 \`pv_search\` 查到` };
  }
  else if (/\[改提醒:([^\]]+)\]/.test(user) && tools === 0) message = { role: "assistant", content: null, tool_calls: [{ id: "call_u", type: "function", function: { name: "pv_propose", arguments: JSON.stringify({ kind: "update_reminder", payload: { item_id: user.match(/\[改提醒:([^\]]+)\]/)[1], title: "【合成】交差旅报销（改）" } }) } }] };
  else if (/\[改提醒:/.test(user)) message = { role: "assistant", content: "已提出修改，等你批准。" };
  else if (user.includes("[提醒]") && tools === 0) message = { role: "assistant", content: null, tool_calls: [{ id: "call_p", type: "function", function: { name: "pv_propose", arguments: JSON.stringify({ kind: "create_reminder", payload: { title: "【合成】交差旅报销", due: "2030-01-02T09:00:00+08:00" } }) } }] };
  else if (user.includes("[提醒]")) message = { role: "assistant", content: "已提出提醒，等你批准。" };
  else if (/\[备注:([^\]]+)\]/.test(user) && tools === 0) message = { role: "assistant", content: null, tool_calls: [{ id: "call_a", type: "function", function: { name: "pv_annotate", arguments: JSON.stringify({ event_ids: [user.match(/\[备注:([^\]]+)\]/)[1]], notes: "【合成】会前要点" }) } }] };
  else if (/\[备注:/.test(user)) message = { role: "assistant", content: "已存到备注。" };
  else if (user.includes("[慢]")) await new Promise((r) => setTimeout(r, 5000));
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({ id: "mock", object: "chat.completion", created: 1, model: body.model, choices: [{ index: 0, message, finish_reason: message.tool_calls ? "tool_calls" : "stop" }], usage: { prompt_tokens: 10, completion_tokens: 2 } }));
});
server.listen(0, "127.0.0.1", () => console.log(JSON.stringify({ port: server.address().port })));
// With MOCK_MODEL_EXIT_ON_EOF=1 the parent keeps our stdin open; when it exits (even if it crashed or was killed)
// stdin closes and so do we, so an aborted test run never leaves this server holding its output pipes.
if (process.env.MOCK_MODEL_EXIT_ON_EOF === "1") {
  process.stdin.on("end", () => process.exit(0));
  process.stdin.on("error", () => process.exit(0));
  process.stdin.resume();
}
