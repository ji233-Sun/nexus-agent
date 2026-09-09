export default function (pi) {
  pi.on("session_start", async (_event, ctx) => {
    ctx.ui.notify("nexus-permissions-ready", "info");
  });
  pi.on("tool_call", async (event, ctx) => {
    if (nexusPermission === "yolo") return;
    if (["read", "grep", "find", "ls"].includes(event.toolName)) return;
    if (nexusPermission === "auto_edit" && ["write", "edit"].includes(event.toolName)) return;
    if (!ctx.hasUI) return { block: true, reason: "Nexus approval is unavailable" };
    const choice = await ctx.ui.select(
      `Allow tool: ${event.toolName}\n${JSON.stringify(event.input)}`,
      ["Approve", "Deny"],
    );
    if (choice !== "Approve") return { block: true, reason: "Denied in Nexus" };
  });
}
