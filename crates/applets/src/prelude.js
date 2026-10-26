// The `opus` object every applet sees. This file is one function
// expression: `runtime.rs` calls it with `host(op, payload)`, the native
// function that runs the Rust side of every call (payload and answer travel
// as JSON, a failure is thrown as an Error), and `init`, the applet's
// identity and settings. It returns the hooks Rust uses to call back into
// the applet (timers, async answers, settings, handlers, redraw).
//
// Timers, async calls and handlers live here, in JavaScript: Rust only keeps
// their numeric ids, so no JavaScript function is ever held by a glib source.
(function (host, init) {
    "use strict";

    /**
     * Run one host call.
     * @param {string} op the operation name, see `OPS` in runtime.rs
     * @param {*} [payload] JSON-able arguments
     * @returns {*} the JSON answer
     */
    const call = (op, payload) => host(op, payload === undefined ? null : payload);

    /**
     * Turn any value into the text of a log line.
     * @param {*} value
     * @returns {string}
     */
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

    /**
     * Join log arguments with spaces, like a browser console.
     * @param {Array<*>} values
     * @returns {string}
     */
    const line = (values) => values.map(text).join(" ");

    /** Log a line (`Event::Log`). */
    const log = (...values) => { call("log", { text: line(values) }); };
    /** Log a warning: a `Log` line prefixed by `warn: `. */
    const warn = (...values) => { call("log", { text: "warn: " + line(values) }); };
    /** Log an error (`Event::Error`). */
    const error = (...values) => { call("error", { text: line(values) }); };

    // ---- timers --------------------------------------------------------
    const timers = new Map();
    let nextTimer = 1;

    /**
     * A delay in whole milliseconds; anything not a positive number is 0.
     * @param {*} ms
     * @returns {number}
     */
    const delay = (ms) => {
        const n = Number(ms);
        return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
    };

    /**
     * Register a timer and ask Rust to fire it on the main loop.
     * @param {Function} fn the callback
     * @param {*} ms the delay
     * @param {Array<*>} args extra arguments for the callback
     * @param {boolean} repeat true for `setInterval`
     * @returns {number} the timer id
     */
    const addTimer = (fn, ms, args, repeat) => {
        if (typeof fn !== "function") throw new TypeError((repeat ? "setInterval" : "setTimeout") + ": the callback must be a function");
        const id = nextTimer++;
        timers.set(id, { fn, args, repeat });
        call("timer.set", { id, ms: delay(ms), repeat });
        return id;
    };

    /**
     * Cancel a timer; an unknown id does nothing, like in a browser.
     * @param {*} id
     */
    const clearTimer = (id) => {
        const n = Number(id);
        if (!timers.has(n)) return;
        timers.delete(n);
        call("timer.clear", { id: n });
    };

    // ---- async calls -----------------------------------------------------
    // A call whose answer comes later (`http.get`, `notify`): Rust runs it on
    // a worker thread and settles the promise through `hooks.settle`.
    const pending = new Map();
    let nextRequest = 1;

    /**
     * Start an async host call.
     * @param {string} op
     * @param {Object} payload
     * @returns {Promise<*>} settled with the answer, rejected with an Error
     */
    const later = (op, payload) => new Promise((resolve, reject) => {
        const request = nextRequest++;
        pending.set(request, { resolve, reject });
        try {
            call(op, { request, payload });
        } catch (e) {
            pending.delete(request);
            reject(e);
        }
    });

    // ---- handlers and internal hooks ------------------------------------
    const handlers = new Map();
    let nextHandler = 1;

    /**
     * For the widget and canvas code (Tasks 6-7), not for applets: the
     * registry of handlers Rust calls with `Runtime::dispatch`, and the
     * `redraw` hook `Runtime::redraw` calls (a no-op until the canvas
     * fills it).
     */
    const internal = {
        /**
         * Register `fn` as a handler.
         * @param {Function} fn
         * @returns {number} the handler id
         */
        registerHandler(fn) {
            if (typeof fn !== "function") throw new TypeError("registerHandler: the handler must be a function");
            const id = nextHandler++;
            handlers.set(id, fn);
            return id;
        },
        /**
         * Forget a handler.
         * @param {number} id
         */
        unregisterHandler(id) { handlers.delete(id); },
        /**
         * Paint canvas `id`; replaced by the canvas code.
         * @param {string} id
         */
        redraw(id) {},
    };

    // ---- settings ----------------------------------------------------------
    let settings = Object.freeze(init.settings || {});
    let onSettings = null;

    // ---- the opus object ---------------------------------------------------
    const fs = Object.freeze({
        /** Whether `path` exists. */
        exists: (path) => call("fs.exists", { path: String(path) }),
        /** The text of the file `path`; throws an Error when it cannot be read. */
        readText: (path) => call("fs.readText", { path: String(path) }),
        /** The entries of the folder `path`, sorted by name: `{ name, path, isFile, isDir, size }`. */
        list: (path) => call("fs.list", { path: String(path) }),
    });

    const http = Object.freeze({
        /**
         * HTTP GET on a worker thread. `options`: `{ headers, timeoutMs }`.
         * @returns {Promise<{status: number, ok: boolean, headers: Object, body: string}>}
         */
        get: (url, options) => {
            const o = options || {};
            return later("http.get", { url: String(url), headers: o.headers || null, timeoutMs: o.timeoutMs === undefined ? null : o.timeoutMs });
        },
    });

    const applet = Object.freeze({
        id: init.id,
        dir: init.dir,
        name: init.name,
        /** The current settings (a frozen object). */
        get settings() { return settings; },
        /**
         * Call `fn(settings)` when the user changes the settings; the last
         * registration wins, `null` removes it.
         * @param {Function|null} fn
         */
        onSettings(fn) {
            if (fn !== null && typeof fn !== "function") throw new TypeError("onSettings: the argument must be a function or null");
            onSettings = fn;
        },
        /**
         * Open an `http`, `https` or `file` URL with the desktop's default handler.
         * @param {string} url
         */
        open: (url) => { call("applet.open", { url: String(url) }); },
        /**
         * The absolute path of `rel` inside the applet folder; throws when it leaves the folder.
         * @param {string} rel
         * @returns {string}
         */
        assetPath: (rel) => call("applet.assetPath", { path: String(rel) }),
        /**
         * The text of the file `rel` inside the applet folder.
         * @param {string} rel
         * @returns {string}
         */
        readAsset: (rel) => call("applet.readAsset", { path: String(rel) }),
    });

    const opus = Object.freeze({
        version: init.version,
        log,
        /**
         * Show a desktop notification. `options`: `{ body, icon, timeout (ms, 0 = never), urgency: "low" | "normal" | "critical" }`.
         * @returns {Promise<number>} the notification id
         */
        notify(title, options) {
            const o = options || {};
            return later("notify", { title: text(title), body: o.body === undefined ? "" : text(o.body), icon: o.icon || "", timeout: o.timeout === undefined ? -1 : o.timeout, urgency: o.urgency || "normal" });
        },
        fs,
        http,
        applet,
    });

    const hidden = (name, value) => Object.defineProperty(globalThis, name, { value, writable: false, enumerable: false, configurable: false });
    hidden("opus", opus);
    hidden("console", Object.freeze({ log, info: log, debug: log, warn, error }));
    hidden("setTimeout", (fn, ms, ...args) => addTimer(fn, ms, args, false));
    hidden("setInterval", (fn, ms, ...args) => addTimer(fn, ms, args, true));
    hidden("clearTimeout", clearTimer);
    hidden("clearInterval", clearTimer);
    hidden("__opusInternal", internal);

    // What Rust calls; it is never visible to the applet.
    return Object.freeze({
        /**
         * Run timer `id`. A timeout is forgotten before its callback runs.
         * @param {number} id
         * @returns {*} what the callback returned (a promise for an async one)
         */
        fireTimer(id) {
            const t = timers.get(id);
            if (!t) throw new Error("timer " + id + " fired but is not registered");
            if (!t.repeat) timers.delete(id);
            return t.fn(...t.args);
        },
        /**
         * Settle the promise of async call `request`.
         * @param {number} request
         * @param {boolean} ok
         * @param {*} value the answer, or the error message when not ok
         */
        settle(request, ok, value) {
            const p = pending.get(request);
            if (!p) throw new Error("no pending call " + request);
            pending.delete(request);
            if (ok) p.resolve(value);
            else p.reject(new Error(String(value)));
        },
        /**
         * Store new settings and tell the `onSettings` callback.
         * @param {Object} next
         */
        settings(next) {
            settings = Object.freeze(next);
            if (onSettings) return onSettings(settings);
        },
        /**
         * Call handler `id` with `arg`.
         * @param {number} id
         * @param {*} arg
         */
        dispatch(id, arg) {
            const h = handlers.get(id);
            if (!h) throw new Error("no handler " + id);
            return h(arg);
        },
        /**
         * Ask the canvas code to paint canvas `id`.
         * @param {string} id
         */
        redraw(id) { return internal.redraw(id); },
    });
})
