import { Plugin } from "@opencode/plugin"

/**
 * Run this project's Rust builds and tests on the remote Docker builder.
 *
 * Every shell call that starts with `cargo build|check|test|clippy|fmt` is
 * rewritten to `scripts/remote-ci.sh ...`, so the laptop only rsyncs the tree
 * and streams output while compilation and tests run on the server.
 *
 * Escape hatch: `command cargo test` does not match and stays local.
 */
const REMOTE_SCRIPT = "scripts/remote-ci.sh"
const REMOTE_VERBS = new Set(["build", "check", "test", "clippy", "fmt", "nextest"])
const REMOTE_TIMEOUT_MS = 60 * 60 * 1000

export default Plugin.define({
  id: "rstorrent.remote-ci",
  async setup(ctx) {
    await ctx.shell.hook("create.before", (event) => {
      const command = event.command?.trim()
      if (!command) return
      if (command.includes(REMOTE_SCRIPT)) return

      // Leading environment assignments are kept out of scope on purpose: an
      // env-prefixed cargo call is treated as explicit local intent.
      const match = command.match(/^cargo\s+(?:--?[^\s]+\s+)*([a-z][a-z-]*)\b/)
      if (!match || !REMOTE_VERBS.has(match[1])) return

      event.command = `${REMOTE_SCRIPT} ${command}`
      event.timeout = Math.max(event.timeout ?? 0, REMOTE_TIMEOUT_MS)
    })
  },
})
