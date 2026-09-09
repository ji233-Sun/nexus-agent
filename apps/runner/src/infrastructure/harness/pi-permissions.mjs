// Loaded explicitly by Nexus; fail closed when no UI is available.
export default function (pi) {
    pi.on("tool_call", async (event, ctx) => {
        if (mode === "yolo") return;
        if (["read", "grep", "find", "ls"].includes(event.toolName)) return;
        if (mode === "auto_edit" && ["write", "edit"].includes(event.toolName)) return;
        if (!ctx.hasUI || !(await ctx.ui.confirm(`Allow ${event.toolName}?`, JSON.stringify(event.input)))) {
            return { block: true, reason: "Action denied by Nexus permission policy." };
        }
    });
}
