/**
 * harness-sync-lock: one harness writes a conversation at a time.
 *
 * Installed identically in ~/.omp/agent/extensions and ~/.pi/agent/extensions
 * (same extension API in both engines; they publish types under different
 * package names, so the structural subset this file needs is declared here
 * verbatim from `extensibility/extensions/types.ts` rather than imported).
 *
 * The lock itself is the `hslock` binary over lock files; this file only
 * decides WHEN to call it:
 *
 *   input        -> interactive only: if another harness holds this
 *                   conversation, swallow the prompt and say who holds it.
 *   turn_start   -> claim. Fires in EVERY mode (-p, rpc, interactive) before
 *                   the model request is built; if the claim fails here the
 *                   turn is aborted via ctx.abort(). This is the real gate;
 *                   `input` only exists to give a nicer message in the TUI.
 *   turn_end     -> release. Emitted synchronously after every turn (the
 *                   assistant message is written by then). agent_end is the
 *                   belt to this brace: it is routed through a maintenance
 *                   path and was observed NOT to reach extensions in the TUI.
 *   session_switch / session_shutdown -> release whatever we held
 *
 * If hslock is missing or fails, we fall OPEN: the prompt proceeds and a
 * one-line warning says the turn is unprotected and how to fix it. The
 * daemon's divergence guard still turns the worst case into a fork.
 */
import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync } from "node:fs";
import { basename, join } from "node:path";
import { homedir } from "node:os";

// ---- structural subset of the harness extension API (omp 18 / pi 1.1) ----

interface ReadonlySessionManager {
	getSessionFile(): string | undefined;
	getSessionId(): string;
}

interface ExtensionUIContext {
	notify(message: string, type?: "info" | "warning" | "error"): void;
}

interface ExtensionContext {
	ui: ExtensionUIContext;
	sessionManager: ReadonlySessionManager;
	/** Stop the current prompt. Available in every mode (interactive, -p, rpc). */
	abort: () => void;
}

interface InputEvent {
	type: "input";
	text: string;
	source: "interactive" | "rpc" | "extension";
}

interface InputEventResult {
	handled?: boolean;
}

interface AgentEndEvent {
	type: "agent_end";
	willContinue?: boolean;
}

interface LifecycleEvent {
	type: "turn_start" | "turn_end" | "session_stop" | "session_switch" | "session_shutdown";
}

interface MessageEndEvent {
	type: "message_end";
	message: { role: string };
}

type Handler<E, R = void> = (event: E, ctx: ExtensionContext) => Promise<R | void> | R | void;

interface ExtensionAPI {
	on(event: "input", handler: Handler<InputEvent, InputEventResult>): void;
	on(event: "turn_start", handler: Handler<LifecycleEvent>): void;
	on(event: "turn_end", handler: Handler<LifecycleEvent>): void;
	on(event: "message_end", handler: Handler<MessageEndEvent>): void;
	on(event: "session_stop", handler: Handler<LifecycleEvent>): void;
	on(event: "agent_end", handler: Handler<AgentEndEvent>): void;
	on(event: "session_switch", handler: Handler<LifecycleEvent>): void;
	on(event: "session_shutdown", handler: Handler<LifecycleEvent>): void;
}

// ---- lock plumbing ----

const HARNESS: "omp" | "pi" = (() => {
	const dir = process.env.PI_CODING_AGENT_DIR ?? "";
	const argv1 = process.argv[1] ?? "";
	if (dir.includes(`${".pi"}`) || (argv1.includes("pi-coding-agent") && !argv1.includes("oh-my-pi"))) return "pi";
	return "omp";
})();
const HSLOCK = process.env.HSLOCK ?? join(homedir(), "bin", "hslock.exe");

function conversationId(sm: ReadonlySessionManager): string | null {
	// Lock on the session uuid. omp and Pi share ids for the same conversation
	// (the Pi copy of an omp journal keeps its id), so one lock covers both.
	const file = sm.getSessionFile();
	if (!file) return null;
	const name = basename(file);
	const m = name.match(/_([0-9a-f-]{36})\.jsonl$/i) ?? name.match(/^([0-9a-f-]{36})\.jsonl$/i);
	return m ? m[1] : sm.getSessionId() || null;
}

interface LockResult {
	code: number;
	holder: string;
}

function hslock(...args: string[]): LockResult | null {
	if (!existsSync(HSLOCK)) return null;
	// No `timeout`: under Bun on Windows it fails with a spurious ETIMEDOUT
	// after ~14 ms (observed). The binary only touches local files and
	// returns in ~5 ms; there is nothing for a timeout to protect against.
	const r = spawnSync(HSLOCK, args, { encoding: "utf8", windowsHide: true });
	if (r.error || r.status === null) {
		trace(`hslock ${args[0]} failed: ${r.error ? String(r.error) : `status=null signal=${String(r.signal)}`}`);
		return null;
	}
	return { code: r.status, holder: (r.stdout ?? "").trim() };
}

const TRACE = process.env.HARNESS_SYNC_TRACE;
function trace(line: string): void {
	if (!TRACE) return;
	try {
		appendFileSync(TRACE, `${new Date().toISOString()} ${HARNESS} ${process.pid} ${line}\n`);
	} catch {
		/* tracing must never break the harness */
	}
}

export default function (pi: ExtensionAPI): void {
	let held: string | null = null;
	let warnedMissing = false;

	const releaseHeld = (): void => {
		trace(`release held=${held}`);
		if (held) {
			const r = hslock("release", held, HARNESS, String(process.pid));
			trace(`release result=${r ? `${r.code} ${r.holder}` : "null"}`);
			// Only forget the lock once the binary confirmed it is gone (0) or
			// says someone else owns it (2). On a spawn failure keep `held` so
			// the next lifecycle event retries instead of leaking the lock.
			if (r && (r.code === 0 || r.code === 2)) held = null;
		}
	};

	pi.on("input", (_event, ctx) => {
		const id = conversationId(ctx.sessionManager);
		if (!id) return;
		const r = hslock("check", id);
		if (r === null) {
			if (!warnedMissing) {
				warnedMissing = true;
				ctx.ui.notify(
					`harness-sync: lock binary not found at ${HSLOCK}; this conversation is unprotected. Build it: cd ~/.local/share/harness-sync/lock && cargo build --release && cp target/release/hslock.exe ~/bin/`,
					"warning",
				);
			}
			return; // fail open
		}
		if (r.code === 2) {
			const [who, pid] = r.holder.split(" ");
			if (!(who === HARNESS && pid === String(process.pid))) {
				ctx.ui.notify(
					`This conversation is being written by ${who} (pid ${pid}). Finish the turn there first; your prompt was not sent.`,
					"error",
				);
				return { handled: true };
			}
		}
	});

	pi.on("turn_start", (_event, ctx) => {
		const id = conversationId(ctx.sessionManager);
		if (!id) return;
		const r = hslock("claim", id, HARNESS, String(process.pid), ctx.sessionManager.getSessionFile() ?? "");
		trace(`turn_start claim=${r ? r.code : "nobin"}`);
		if (r === null) return; // binary missing: fail open (warned in `input`)
		if (r.code === 0) {
			held = id;
			return;
		}
		const [who, pid] = r.holder.split(" ");
		ctx.ui.notify(
			`harness-sync: conversation is being written by ${who} (pid ${pid}); this turn was stopped. Finish the turn there first.`,
			"error",
		);
		ctx.abort();
	});

	pi.on("turn_end", () => {
		trace("turn_end");
		releaseHeld();
	});
	pi.on("agent_end", (event) => {
		trace(`agent_end willContinue=${String(event.willContinue)}`);
		if (event.willContinue) return; // auto-retry pending: still our turn
		releaseHeld();
	});
	// Observed: one long streamed turn left the lock held although later
	// short turns released fine. message_end fires when the assistant
	// message is complete regardless of how the turn settles, and
	// session_stop is the engine's own "run is over" control hook, so release
	// on both as well. Redundant in the normal case, decisive in the odd one.
	pi.on("message_end", (event) => {
		trace(`message_end role=${event.message.role}`);
		// Fires for the USER message too (0.1 s after turn_start); only the
		// assistant's completed message means the turn's writes are done.
		if (event.message.role === "assistant") releaseHeld();
	});
	pi.on("session_stop", () => {
		trace("session_stop");
		releaseHeld();
	});

	pi.on("session_switch", () => {
		trace("session_switch");
		releaseHeld();
	});
	pi.on("session_shutdown", () => {
		trace("session_shutdown");
		releaseHeld();
	});
}
