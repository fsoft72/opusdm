// The `ctx` an applet's canvas `draw(ctx)` receives: a recorder with the
// Canvas 2D names that turns every call and style change into a plain JSON
// command (`{ op: "fillRect", x, y, w, h }`, see `canvas.rs`). Nothing is
// painted here: the prelude sends the command list to Rust, and the GTK draw
// function replays it with cairo. This file is one function expression;
// `runtime.rs` evaluates it and hands its result to the prelude.
(function () {
    "use strict";

    /** The state of a new canvas, as in browsers. */
    const DEFAULTS = Object.freeze({
        fillStyle: "#000000",
        strokeStyle: "#000000",
        lineWidth: 1,
        lineCap: "butt",
        globalAlpha: 1,
        font: "10px sans-serif",
        textAlign: "start",
        textBaseline: "alphabetic",
    });

    /** The key of the JSON maker hidden on a gradient object. */
    const GRADIENT = Symbol("gradient");

    /**
     * A number for a command: JSON has no NaN or infinity, so those travel
     * as `null` (Rust then skips the command, as browsers do).
     * @param {*} value
     * @returns {number|null}
     */
    const num = (value) => {
        const n = Number(value);
        return Number.isFinite(n) ? n : null;
    };

    /**
     * A gradient object with `addColorStop`; its stops are read when it is
     * assigned to `fillStyle` / `strokeStyle`.
     * @param {Object} spec the JSON of the gradient without its stops
     * @returns {Object}
     */
    const makeGradient = (spec) => {
        const stops = [];
        const gradient = {
            /**
             * Add a colour stop.
             * @param {number} offset from 0 to 1
             * @param {string} color a CSS colour
             */
            addColorStop(offset, color) { stops.push({ offset: num(offset), color: String(color) }); },
        };
        Object.defineProperty(gradient, GRADIENT, { value: () => Object.assign({}, spec, { stops: stops.slice() }) });
        return Object.freeze(gradient);
    };

    /**
     * The JSON of a fill or stroke style: a gradient's spec, or the text of a colour.
     * @param {*} value
     * @returns {Object|string}
     */
    const styleJson = (value) => (value !== null && typeof value === "object" && typeof value[GRADIENT] === "function" ? value[GRADIENT]() : String(value));

    /**
     * A new recorder for a canvas of `width` x `height` pixels.
     * @param {number} width
     * @param {number} height
     * @param {number} max the most commands one draw may record
     * @returns {{ctx: Object, commands: Array<Object>, close: Function}}
     */
    const recorder = (width, height, max) => {
        const commands = [];
        let closed = false;
        let state = Object.assign({}, DEFAULTS);
        const stack = [];

        /**
         * Record one command.
         * @param {Object} command
         */
        const push = (command) => {
            if (closed) throw new Error("canvas: this ctx belongs to a draw that has already finished");
            if (commands.length >= max) throw new RangeError("canvas: a draw may record at most " + max + " commands");
            commands.push(command);
        };

        const ctx = {};
        /**
         * Define the read-only value `name` of `ctx`.
         * @param {string} name
         * @param {*} value
         */
        const fixed = (name, value) => Object.defineProperty(ctx, name, { value, enumerable: true });

        /**
         * Define the style property `name`: reading gives the last value set
         * (`accept` decides which values are kept, as browsers ignore some),
         * writing records `command(value)`.
         * @param {string} name
         * @param {Function} command
         * @param {Function} [accept]
         */
        const style = (name, command, accept) => Object.defineProperty(ctx, name, {
            enumerable: true,
            get: () => state[name],
            set: (value) => {
                push(command(value));
                if (!accept || accept(value)) state[name] = value;
            },
        });

        fixed("width", width);
        fixed("height", height);
        fixed("canvas", Object.freeze({ width, height }));

        /** Push the state; `restore` brings it back. */
        fixed("save", () => {
            push({ op: "save" });
            stack.push(Object.assign({}, state));
        });
        /** Pop the state pushed by `save`; with no `save` it does nothing. */
        fixed("restore", () => {
            push({ op: "restore" });
            if (stack.length > 0) state = stack.pop();
        });
        /** Move the origin. */
        fixed("translate", (x, y) => push({ op: "translate", x: num(x), y: num(y) }));
        /** Rotate by `angle` radians, clockwise. */
        fixed("rotate", (angle) => push({ op: "rotate", angle: num(angle) }));
        /** Scale the axes. */
        fixed("scale", (x, y) => push({ op: "scale", x: num(x), y: num(y) }));
        /** Start a new path. */
        fixed("beginPath", () => push({ op: "beginPath" }));
        /** Close the current sub-path. */
        fixed("closePath", () => push({ op: "closePath" }));
        /** Start a sub-path at (x, y). */
        fixed("moveTo", (x, y) => push({ op: "moveTo", x: num(x), y: num(y) }));
        /** Add a line to (x, y). */
        fixed("lineTo", (x, y) => push({ op: "lineTo", x: num(x), y: num(y) }));
        /** Add an arc of a circle. */
        fixed("arc", (x, y, radius, startAngle, endAngle, counterclockwise) => push({
            op: "arc", x: num(x), y: num(y), radius: num(radius), startAngle: num(startAngle), endAngle: num(endAngle), counterclockwise: Boolean(counterclockwise),
        }));
        /** Add a rectangle to the path. */
        fixed("rect", (x, y, w, h) => push({ op: "rect", x: num(x), y: num(y), w: num(w), h: num(h) }));
        /** Fill the current path with `fillStyle`. */
        fixed("fill", () => push({ op: "fill" }));
        /** Stroke the current path with `strokeStyle` and `lineWidth`. */
        fixed("stroke", () => push({ op: "stroke" }));
        /** Fill a rectangle (the current path is not changed). */
        fixed("fillRect", (x, y, w, h) => push({ op: "fillRect", x: num(x), y: num(y), w: num(w), h: num(h) }));
        /** Stroke a rectangle (the current path is not changed). */
        fixed("strokeRect", (x, y, w, h) => push({ op: "strokeRect", x: num(x), y: num(y), w: num(w), h: num(h) }));
        /** Make a rectangle transparent. */
        fixed("clearRect", (x, y, w, h) => push({ op: "clearRect", x: num(x), y: num(y), w: num(w), h: num(h) }));

        /**
         * A text command, with `maxWidth` only when given.
         * @param {string} op
         * @returns {Function}
         */
        const textCommand = (op) => (text, x, y, maxWidth) => {
            const command = { op, text: String(text), x: num(x), y: num(y) };
            if (maxWidth !== undefined) command.maxWidth = num(maxWidth);
            push(command);
        };
        /** Fill `text` at (x, y) with `font`, `textAlign`, `textBaseline`. */
        fixed("fillText", textCommand("fillText"));
        /** Stroke `text` at (x, y). */
        fixed("strokeText", textCommand("strokeText"));

        /**
         * Draw the image file `path` (absolute, from `opus.applet.assetPath`)
         * at (x, y), scaled to w x h when given.
         */
        fixed("drawImage", (path, x, y, w, h) => {
            if (typeof path !== "string") throw new TypeError("drawImage: the image must be a file path from opus.applet.assetPath");
            const command = { op: "drawImage", path, x: num(x), y: num(y) };
            if (w !== undefined || h !== undefined) {
                command.w = num(w);
                command.h = num(h);
            }
            push(command);
        });
        /** A linear gradient from (x0, y0) to (x1, y1). */
        fixed("createLinearGradient", (x0, y0, x1, y1) => makeGradient({ type: "linear", x0: num(x0), y0: num(y0), x1: num(x1), y1: num(y1) }));
        /** A radial gradient between two circles. */
        fixed("createRadialGradient", (x0, y0, r0, x1, y1, r1) => makeGradient({ type: "radial", x0: num(x0), y0: num(y0), r0: num(r0), x1: num(x1), y1: num(y1), r1: num(r1) }));

        style("fillStyle", (v) => ({ op: "fillStyle", style: styleJson(v) }));
        style("strokeStyle", (v) => ({ op: "strokeStyle", style: styleJson(v) }));
        style("lineWidth", (v) => ({ op: "lineWidth", value: num(v) }), (v) => num(v) !== null && Number(v) > 0);
        style("lineCap", (v) => ({ op: "lineCap", value: String(v) }));
        style("globalAlpha", (v) => ({ op: "globalAlpha", value: num(v) }), (v) => num(v) !== null && Number(v) >= 0 && Number(v) <= 1);
        style("font", (v) => ({ op: "font", value: String(v) }));
        style("textAlign", (v) => ({ op: "textAlign", value: String(v) }));
        style("textBaseline", (v) => ({ op: "textBaseline", value: String(v) }));

        return {
            ctx,
            commands,
            /** End this draw: any later call on `ctx` throws. */
            close() { closed = true; },
        };
    };

    return Object.freeze({ recorder });
})
