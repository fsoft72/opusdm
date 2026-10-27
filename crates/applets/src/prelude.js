// The `opus` object every applet sees. This file is one function
// expression: `runtime.rs` calls it with `host(op, payload)`, the native
// function that runs the Rust side of every call (payload and answer travel
// as JSON, a failure is thrown as an Error), `init`, the applet's identity
// and settings, and `canvas`, what `canvas.js` returned (the `ctx` recorder).
// It returns the hooks Rust uses to call back into
// the applet (timers, async answers, settings, handlers, redraw).
//
// Timers, async calls and handlers live here, in JavaScript: Rust only keeps
// their numeric ids, so no JavaScript function is ever held by a glib source.
(function (host, init, canvas) {
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
         * Paint canvas `id` of the shown tree: run its `draw` now and send
         * the commands it recorded. Throws for an unknown canvas.
         * @param {string} id
         */
        redraw(id) { paint(String(id)); },
    };

    // ---- widgets -----------------------------------------------------------
    // `opus.ui.show` sends the tree to Rust with every function replaced by
    // `{ handler: <id> }`. The handlers of the shown tree are all in `shown`;
    // those of a node with an id are also kept by "id\u0000prop", so an
    // update of that prop can unregister the ones it replaces.
    const MAX_TREE_DEPTH = 64;
    let shown = new Set();
    let propHandlers = new Map();

    // ---- canvases ------------------------------------------------------------
    // The canvases of the shown tree, by id: `{ handler, width, height }`
    // (`handler` is the id of the `draw` function, or null). A canvas with no
    // id gets one, `__opusCanvas<n>`, so `show` can still paint it.
    const MAX_CANVAS_COMMANDS = 100000;
    let canvases = new Map();
    let nextCanvas = 1;

    /**
     * Run the `draw` of canvas `id` with a fresh recorder and send what it
     * recorded (`Event::Redraw`). A draw that throws sends nothing, so the
     * previous picture stays; the error goes to the caller.
     * @param {string} id
     */
    const paint = (id) => {
        const c = canvases.get(id);
        if (!c) throw new Error("opus.ui.redraw: no canvas \"" + id + "\" in the shown tree");
        const rec = canvas.recorder(c.width, c.height, MAX_CANVAS_COMMANDS);
        try {
            if (c.handler !== null) {
                const draw = handlers.get(c.handler);
                if (!draw) throw new Error("canvas \"" + id + "\": its draw handler " + c.handler + " is not registered");
                const result = draw(rec.ctx);
                if (result !== null && typeof result === "object" && typeof result.then === "function") {
                    warn("canvas \"" + id + "\": draw returned a promise; it must draw synchronously, what it draws after an await is lost");
                }
            }
        } finally {
            rec.close();
        }
        call("canvas.redraw", { id, commands: rec.commands });
    };

    /**
     * Copy `value` with every function registered and replaced by
     * `{ handler: id }`; the new ids are pushed to `added`.
     * @param {*} value
     * @param {Array<number>} added
     * @param {number} depth
     * @returns {*} the JSON-able copy
     */
    const encode = (value, added, depth) => {
        if (depth > MAX_TREE_DEPTH) throw new RangeError("opus.ui: the tree is nested deeper than " + MAX_TREE_DEPTH + " levels (is there a cycle?)");
        if (typeof value === "function") {
            const id = internal.registerHandler(value);
            added.push(id);
            return { handler: id };
        }
        if (Array.isArray(value)) return value.map((v) => encode(v, added, depth + 1));
        if (value === null || typeof value !== "object") return value;
        const copy = {};
        for (const key of Object.keys(value)) copy[key] = encode(value[key], added, depth + 1);
        return copy;
    };

    /**
     * Encode the node `node` and its `children`; the handlers of a node
     * with an id are recorded in `keyed` by "id\u0000prop", the canvases in
     * `found` (a canvas with no id gets a generated one).
     * @param {*} node
     * @param {Array<number>} added
     * @param {Map<string, Array<number>>} keyed
     * @param {Map<string, Object>} found
     * @param {number} depth
     * @returns {*} the JSON-able node
     */
    const encodeNode = (node, added, keyed, found, depth) => {
        if (depth > MAX_TREE_DEPTH) throw new RangeError("opus.ui: the tree is nested deeper than " + MAX_TREE_DEPTH + " levels (is there a cycle?)");
        if (node === null || typeof node !== "object" || Array.isArray(node)) return encode(node, added, depth);
        const copy = {};
        for (const key of Object.keys(node)) {
            const value = node[key];
            if (key === "children" && Array.isArray(value)) {
                copy[key] = value.map((child) => encodeNode(child, added, keyed, found, depth + 1));
                continue;
            }
            const ids = [];
            copy[key] = encode(value, ids, depth + 1);
            added.push(...ids);
            if (typeof node.id === "string" && ids.length > 0) keyed.set(node.id + "\u0000" + key, ids);
        }
        if (node.type === "canvas") {
            if (copy.id === undefined || copy.id === null) copy.id = "__opusCanvas" + nextCanvas++;
            const draw = copy.draw;
            const handler = draw !== null && typeof draw === "object" && typeof draw.handler === "number" ? draw.handler : null;
            if (typeof copy.id === "string") found.set(copy.id, { handler, width: Number(node.width), height: Number(node.height) });
        }
        return copy;
    };

    /**
     * Forget the handlers `ids`.
     * @param {Array<number>|Set<number>} ids
     */
    const forget = (ids) => { for (const id of ids) internal.unregisterHandler(id); };

    /**
     * Throw a TypeError unless `value` is a plain object.
     * @param {*} value
     * @param {string} message
     */
    const needObject = (value, message) => {
        if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError(message);
    };

    const ui = Object.freeze({
        /**
         * Show `tree` in the applet window, replacing the tree shown before
         * (its handlers are forgotten), then paint every canvas of it. A node
         * is `{ type, id?, class?, visible?, ...props }`; see the applet docs
         * for the types. A canvas whose draw throws is reported and the
         * others still paint.
         * @param {Object} tree
         */
        show(tree) {
            needObject(tree, "opus.ui.show: the tree must be a node object");
            const added = [];
            const keyed = new Map();
            const found = new Map();
            let encoded;
            try {
                encoded = encodeNode(tree, added, keyed, found, 1);
            } catch (e) {
                forget(added);
                throw e;
            }
            forget(shown);
            shown = new Set(added);
            propHandlers = keyed;
            canvases = found;
            call("ui.show", { tree: encoded });
            for (const id of found.keys()) {
                try {
                    paint(id);
                } catch (e) {
                    error("canvas \"" + id + "\": " + text(e));
                }
            }
        },
        /**
         * Change props of the node `id`. A function prop replaces the
         * handlers that prop had (they are forgotten).
         * @param {string} id
         * @param {Object} props
         */
        update(id, props) {
            if (typeof id !== "string" || id === "") throw new TypeError("opus.ui.update: the id must be a non-empty string");
            needObject(props, "opus.ui.update: the props must be an object");
            const encoded = {};
            const replaced = [];
            try {
                for (const key of Object.keys(props)) {
                    const ids = [];
                    replaced.push([key, ids]);
                    encoded[key] = encode(props[key], ids, 1);
                }
            } catch (e) {
                for (const [, ids] of replaced) forget(ids);
                throw e;
            }
            for (const [key, ids] of replaced) {
                const slot = id + "\u0000" + key;
                const old = propHandlers.get(slot);
                if (old) {
                    forget(old);
                    for (const h of old) shown.delete(h);
                    propHandlers.delete(slot);
                }
                if (key === "draw" && canvases.has(id)) canvases.get(id).handler = ids.length > 0 ? ids[0] : null;
                if (ids.length === 0) continue;
                propHandlers.set(slot, ids);
                for (const h of ids) shown.add(h);
            }
            call("ui.update", { id, props: encoded });
        },
        /**
         * Paint canvas `canvasId` again: its `draw(ctx)` runs now. Throws
         * when the shown tree has no such canvas.
         * @param {string} canvasId
         */
        redraw(canvasId) { internal.redraw(String(canvasId)); },
    });

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
        /**
         * HTTP POST on a worker thread. `options`: `{ headers, body, timeoutMs }`;
         * `body` is a string sent as is, the script sets `Content-Type`.
         * @returns {Promise<{status: number, ok: boolean, headers: Object, body: string}>}
         */
        post: (url, options) => {
            const o = options || {};
            return later("http.post", {
                url: String(url),
                headers: o.headers || null,
                body: o.body === undefined || o.body === null ? null : String(o.body),
                timeoutMs: o.timeoutMs === undefined ? null : o.timeoutMs,
            });
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
        ui,
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
         * Paint canvas `id` (`Runtime::redraw`).
         * @param {string} id
         */
        redraw(id) { return internal.redraw(id); },
    });
})
