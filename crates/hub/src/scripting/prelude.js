// The `opus` object every script sees. It is built on one native function,
// `__opusHost(op, payload)`, that the hub implements in `scripting.rs`: the
// payload and the answer travel as JSON, a failure is thrown as an Error.
// Everything here is synchronous: a call returns once the hub is done.
(function (host) {
    "use strict";

    const call = (op, payload) => host(op, payload === undefined ? null : payload);

    const text = (value) => {
        if (typeof value === "string") return value;
        if (value === undefined) return "undefined";
        if (value instanceof Error) return value.name + ": " + value.message;
        if (typeof value === "function") return "[function " + (value.name || "anonymous") + "]";
        try {
            const json = JSON.stringify(value);
            return json === undefined ? String(value) : json;
        } catch (e) {
            return String(value);
        }
    };

    const log = (...values) => call("log", { text: values.map(text).join(" ") });

    // ---- paths ---------------------------------------------------------
    const path = Object.freeze({
        sep: "/",
        join(...parts) {
            const joined = parts.filter((p) => p !== undefined && p !== null && p !== "").join("/");
            return path.normalize(joined);
        },
        normalize(p) {
            const absolute = p.startsWith("/");
            const out = [];
            for (const part of p.split("/")) {
                if (part === "" || part === ".") continue;
                if (part === ".." && out.length && out[out.length - 1] !== "..") out.pop();
                else if (part !== ".." || !absolute) out.push(part);
            }
            return (absolute ? "/" : "") + out.join("/") || (absolute ? "/" : ".");
        },
        basename(p, ext) {
            const trimmed = p.replace(/\/+$/, "");
            let base = trimmed.slice(trimmed.lastIndexOf("/") + 1);
            if (ext && base.endsWith(ext) && base !== ext) base = base.slice(0, -ext.length);
            return base;
        },
        dirname(p) {
            const trimmed = p.replace(/\/+$/, "");
            const at = trimmed.lastIndexOf("/");
            if (at < 0) return ".";
            return at === 0 ? "/" : trimmed.slice(0, at);
        },
        extname(p) {
            const base = path.basename(p);
            const at = base.lastIndexOf(".");
            return at <= 0 ? "" : base.slice(at);
        },
    });

    // ---- files ---------------------------------------------------------
    const fs = Object.freeze({
        exists: (p) => call("fs.stat", { path: p }) !== null,
        isFile: (p) => { const s = call("fs.stat", { path: p }); return s !== null && s.isFile; },
        isDir: (p) => { const s = call("fs.stat", { path: p }); return s !== null && s.isDir; },
        stat: (p) => call("fs.stat", { path: p }),
        read: (p) => call("fs.read", { path: p }),
        write: (p, data) => { call("fs.write", { path: p, data: String(data), append: false }); },
        append: (p, data) => { call("fs.write", { path: p, data: String(data), append: true }); },
        list: (dir, pattern) => call("fs.list", { path: dir === undefined ? "." : dir, pattern: pattern || null }),
        mkdir: (p) => { call("fs.mkdir", { path: p }); },
    });

    // ---- http ----------------------------------------------------------
    const http = Object.freeze({
        /**
         * Synchronous HTTP GET. `options`: `{ headers, timeoutMs }`.
         * Returns `{ status, ok, headers, body }`; throws an `Error` on failure.
         */
        get: (url, options) => {
            const o = options || {};
            return call("http.get", { url: String(url), headers: o.headers || null, timeoutMs: o.timeoutMs === undefined ? null : o.timeoutMs });
        },
    });

    // ---- listers -------------------------------------------------------
    // A snapshot of one Lister. `selected` is known for SOURCE and DEST only
    // (the hub collected it when the function started); for the others it is
    // an empty list.
    class Lister {
        constructor(info) {
            this.id = info.id === undefined ? null : info.id;
            this.role = info.role || "none";
            this.path = info.path;
            this.selected = Object.freeze((info.selected || []).slice());
        }
        get isSource() { return this.role === "source"; }
        get isDest() { return this.role === "dest"; }
        get selectedNames() { return this.selected.map((p) => path.basename(p)); }
        get selectedFiles() { return this.selected.filter((p) => fs.isFile(p)); }
        get selectedDirs() { return this.selected.filter((p) => fs.isDir(p)); }
        /** Every entry of the folder, as `opus.fs.list` returns them. */
        files(pattern) { return fs.list(this.path, pattern); }
        /** Show `dir` in this Lister. The SOURCE also makes it the folder of
         *  the next commands. `options`: `{ select: "name", mode: "icons" }`. */
        go(dir, options) {
            const o = options || {};
            if (this.role === "source") {
                const params = { path: dir };
                if (o.mode) params.mode = o.mode;
                command("ChangeDir", params);
                this.path = context().source.path;
                return this;
            }
            const id = this.id !== null ? this.id : lookupId(this.role);
            if (id === null) throw new Error("this Lister is gone");
            this.path = call("lister.go", { id, path: dir }).path;
            return this;
        }
        /** Read the folder again. */
        refresh() { call("refresh", { path: this.path }); return this; }
        toString() { return "[Lister " + this.role + " " + this.path + "]"; }
    }

    const context = () => call("context");

    const lookupId = (role) => {
        const found = call("listers").find((l) => l.role === role);
        return found ? found.id : null;
    };

    const listerOf = (role) => {
        const ctx = context();
        const info = ctx[role];
        if (!info) return null;
        return new Lister({ role, path: info.path, selected: info.selected, id: null });
    };

    // ---- commands ------------------------------------------------------
    const command = (name, params) => {
        if (typeof name !== "string" || name === "") throw new TypeError("opus.command: the command name must be a string");
        const p = Object.assign({}, params || {});
        let files = null;
        if (p.files !== undefined) {
            files = (Array.isArray(p.files) ? p.files : [p.files]).map(String);
            delete p.files;
        }
        let to = null;
        if (p.to !== undefined) {
            to = String(p.to);
            delete p.to;
        }
        call("command", { name, params: p, files, to });
    };

    const bank = (action) => (name) => command(action, { bank: name });

    const start = context();
    const opus = {
        version: call("version"),
        /** The name of the running script. */
        scriptName: start.script,
        /** The text of the step's Arguments field, placeholders expanded. */
        args: start.args,
        home: start.home,
        get source() { return listerOf("source"); },
        get dest() { return listerOf("dest"); },
        /** Every open Lister. */
        get listers() {
            const ctx = context();
            return call("listers").map((l) => {
                const own = ctx[l.role];
                const selected = own && own.path === l.path ? own.selected : [];
                return new Lister({ id: l.id, role: l.role, path: l.path, selected });
            });
        },
        /** The internal commands, with their parameters. */
        get commands() { return call("commands"); },
        /** The saved Functions: `{ id, name }`. */
        get functions() { return call("functions").map((f) => ({ id: f.id, name: f.name })); },
        /** The saved scripts: `{ name, description }`. */
        get scripts() { return call("scripts"); },
        command,
        log,
        print: log,
        /** Ask a yes / no question; true for yes. */
        confirm: (message) => call("confirm", { message: text(message) }),
        /** Show `message` in a modal dialog with one OK button; returns when it is closed. */
        alert: (message) => { call("alert", { message: text(message) }); },
        /** Show a desktop notification. `options`: `{ body, icon, timeout (ms, 0 = never), urgency: "low" | "normal" | "critical" }`.
         *  Returns the notification id. */
        notify(title, options) {
            const o = options || {};
            return call("notify", { title: text(title), body: o.body === undefined ? "" : text(o.body), icon: o.icon || "", timeout: o.timeout === undefined ? -1 : o.timeout, urgency: o.urgency || "normal" });
        },
        /** Start a program (`run("kitty -e htop")`). `options`: `{ wait, window }`. */
        run(commandLine, options) {
            const o = options || {};
            command("Run", { command: commandLine, wait: !!o.wait, window: !!o.window });
        },
        /** Run a program, wait and capture its output: `{ code, stdout, stderr }`.
         *  `options`: `{ cwd, input }`. */
        exec(program, args, options) {
            const o = options || {};
            return call("exec", {
                program: String(program),
                args: (args || []).map(String),
                cwd: o.cwd === undefined ? null : String(o.cwd),
                input: o.input === undefined ? null : String(o.input),
            });
        },
        /** `bash -c script`, captured like `exec`. */
        shell: (script, options) => opus.exec("bash", ["-c", String(script)], options),
        sleep: (ms) => { call("sleep", { ms: Number(ms) || 0 }); },
        env: (name) => call("env", { name: String(name) }),
        newLister: (dir) => command("NewLister", dir === undefined ? {} : { path: dir }),
        setBackground: (file, fit) => command("SetBackground", fit ? { path: file, fit } : { path: file }),
        leaveOut: (files) => command("LeaveOut", files === undefined ? {} : { files }),
        putAway: (files) => command("PutAway", files === undefined ? {} : { files }),
        hide: () => command("Hide"),
        reveal: () => command("Reveal"),
        toggleHide: () => command("ToggleHide"),
        desktop: Object.freeze({ next: () => command("DesktopNext"), prev: () => command("DesktopPrev") }),
        bank: Object.freeze({ open: bank("OpenBank"), close: bank("CloseBank"), toggle: bank("ToggleBank"), hide: bank("HideBank") }),
        /** Run a saved Function, by name or id. */
        runFunction: (nameOrId) => { call("runFunction", { name: String(nameOrId) }); },
        /** Run another script, by name; `args` becomes its `opus.args`. */
        runScript: (name, args) => command("Script", { script: name, args: args === undefined ? "" : String(args) }),
        /** Read `dir` again in every Lister showing it. */
        refresh: (dir) => { call("refresh", { path: dir }); },
        fs,
        http,
        path,
        Lister,
    };

    globalThis.opus = Object.freeze(opus);
    globalThis.console = Object.freeze({ log, info: log, warn: log, error: log, debug: log });
})(__opusHost);
