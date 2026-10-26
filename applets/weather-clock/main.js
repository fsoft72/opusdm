// Weather Clock: an analog clock with a text block below it, drawn on one
// canvas that is redrawn every second. The weather comes from Open-Meteo; the
// clock itself needs no network and draws before (or without) weather data.

/** Canvas size in pixels. */
const WIDTH = 260;
const HEIGHT = 320;
/** Centre and radius of the clock face. */
const CENTER_X = 130;
const CENTER_Y = 112;
const RADIUS = 100;
/** The timeout of one weather request. */
const FETCH_TIMEOUT_MS = 15000;
/** The refresh period when the setting is missing or invalid. */
const DEFAULT_REFRESH_MINUTES = 30;
/** Short weekday names, Sunday first (as `Date.getDay`). */
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** The last weather read, or null: { city, temperature, code, days: [{ day, max, min, code }], unit }. */
let weather = null;
/** A short text when the weather cannot be shown, or "". */
let problem = "";
/** The interval timer of the periodic refresh. */
let refreshTimer = null;
/** Counts fetches: an answer of an older fetch is ignored. */
let fetchSerial = 0;

/**
 * A short text for a WMO weather code.
 * @param {number} code
 * @returns {string}
 */
function describeCode(code) {
    if (code === 0) return "Clear sky";
    if (code <= 2) return "Partly cloudy";
    if (code === 3) return "Overcast";
    if (code === 45 || code === 48) return "Fog";
    if (code >= 51 && code <= 57) return "Drizzle";
    if (code >= 61 && code <= 67) return "Rain";
    if (code >= 71 && code <= 77) return "Snow";
    if (code >= 80 && code <= 82) return "Rain showers";
    if (code === 85 || code === 86) return "Snow showers";
    if (code >= 95) return "Thunderstorm";
    return "Weather code " + code;
}

/**
 * Draw one hand of the clock.
 * @param {Object} ctx
 * @param {number} fraction the position on the dial, 0 to 1 clockwise from 12
 * @param {number} length in pixels
 * @param {number} width line width
 * @param {string} color
 */
function drawHand(ctx, fraction, length, width, color) {
    const angle = fraction * 2 * Math.PI - Math.PI / 2;
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(CENTER_X, CENTER_Y);
    ctx.lineTo(CENTER_X + Math.cos(angle) * length, CENTER_Y + Math.sin(angle) * length);
    ctx.stroke();
}

/**
 * Draw the clock face, the marks and the three hands for `now`.
 * @param {Object} ctx
 * @param {Date} now
 */
function drawClock(ctx, now) {
    const face = ctx.createRadialGradient(CENTER_X - 25, CENTER_Y - 30, 10, CENTER_X, CENTER_Y, RADIUS);
    face.addColorStop(0, "#fdfdfd");
    face.addColorStop(1, "#c9d3e0");
    ctx.fillStyle = face;
    ctx.beginPath();
    ctx.arc(CENTER_X, CENTER_Y, RADIUS, 0, 2 * Math.PI);
    ctx.fill();
    ctx.strokeStyle = "#35465c";
    ctx.lineWidth = 4;
    ctx.stroke();

    ctx.lineCap = "butt";
    for (let hour = 0; hour < 12; hour++) {
        const angle = (hour / 12) * 2 * Math.PI;
        const inner = RADIUS - (hour % 3 === 0 ? 16 : 9);
        ctx.strokeStyle = "#35465c";
        ctx.lineWidth = hour % 3 === 0 ? 3 : 1.5;
        ctx.beginPath();
        ctx.moveTo(CENTER_X + Math.sin(angle) * inner, CENTER_Y - Math.cos(angle) * inner);
        ctx.lineTo(CENTER_X + Math.sin(angle) * (RADIUS - 4), CENTER_Y - Math.cos(angle) * (RADIUS - 4));
        ctx.stroke();
    }

    const seconds = now.getSeconds();
    const minutes = now.getMinutes() + seconds / 60;
    const hours = (now.getHours() % 12) + minutes / 60;
    drawHand(ctx, hours / 12, RADIUS * 0.5, 6, "#1d2b3d");
    drawHand(ctx, minutes / 60, RADIUS * 0.75, 4, "#1d2b3d");
    drawHand(ctx, seconds / 60, RADIUS * 0.85, 1.5, "#c0392b");
    ctx.fillStyle = "#c0392b";
    ctx.beginPath();
    ctx.arc(CENTER_X, CENTER_Y, 5, 0, 2 * Math.PI);
    ctx.fill();
}

/**
 * Draw the text block under the clock: the weather, or why there is none.
 * @param {Object} ctx
 */
function drawWeatherText(ctx) {
    ctx.fillStyle = "rgba(20, 30, 45, 0.78)";
    ctx.fillRect(6, 224, WIDTH - 12, HEIGHT - 228);
    ctx.textAlign = "center";
    ctx.fillStyle = "#ffffff";
    if (!weather) {
        ctx.font = "14px sans-serif";
        ctx.fillText(problem || "Loading weather...", CENTER_X, 275);
        return;
    }
    ctx.font = "bold 16px sans-serif";
    ctx.fillText(weather.city, CENTER_X, 243);
    ctx.font = "bold 22px sans-serif";
    ctx.fillText(Math.round(weather.temperature) + "°" + weather.unit + "  " + describeCode(weather.code), CENTER_X, 265);
    ctx.font = "12px sans-serif";
    weather.days.forEach((day, index) => {
        const text = day.day + "  " + Math.round(day.max) + "° / " + Math.round(day.min) + "°  " + describeCode(day.code);
        ctx.fillText(text, CENTER_X, 284 + index * 13);
    });
}

/**
 * The draw function of the canvas.
 * @param {Object} ctx
 */
function draw(ctx) {
    ctx.clearRect(0, 0, WIDTH, HEIGHT);
    drawClock(ctx, new Date());
    drawWeatherText(ctx);
}

/**
 * A positive whole number setting, or `fallback`.
 * @param {*} value
 * @param {number} fallback
 * @returns {number}
 */
function wholeSetting(value, fallback) {
    const n = Math.floor(Number(value));
    return Number.isFinite(n) && n >= 1 ? n : fallback;
}

/**
 * Get a URL and parse its JSON body; rejects on a non-2xx status.
 * @param {string} url
 * @returns {Promise<Object>}
 */
function getJson(url) {
    return opus.http.get(url, { timeoutMs: FETCH_TIMEOUT_MS }).then((reply) => {
        if (!reply.ok) throw new Error("HTTP " + reply.status);
        return JSON.parse(reply.body);
    });
}

/**
 * The short weekday name of an ISO date "YYYY-MM-DD".
 * @param {string} iso
 * @returns {string}
 */
function weekdayOf(iso) {
    const parts = String(iso).split("-").map(Number);
    return WEEKDAYS[new Date(parts[0], parts[1] - 1, parts[2]).getDay()] || String(iso);
}

/** Fetch the weather of the configured city. */
function refresh() {
    const settings = opus.applet.settings;
    const city = String(settings.city || "").trim();
    const fahrenheit = settings.units === "F";
    const serial = ++fetchSerial;
    if (!city) {
        weather = null;
        problem = "Set a city in the settings";
        return;
    }
    getJson("https://geocoding-api.open-meteo.com/v1/search?name=" + encodeURIComponent(city) + "&count=1").then((geo) => {
        const place = geo.results && geo.results[0];
        if (!place) throw new Error("city not found: " + city);
        const url = "https://api.open-meteo.com/v1/forecast?latitude=" + place.latitude + "&longitude=" + place.longitude
            + "&current=temperature_2m,weather_code&daily=temperature_2m_max,temperature_2m_min,weather_code"
            + "&timezone=auto&forecast_days=3" + (fahrenheit ? "&temperature_unit=fahrenheit" : "");
        return getJson(url).then((data) => ({ place, data }));
    }).then(({ place, data }) => {
        if (serial !== fetchSerial) return;
        const daily = data.daily;
        weather = {
            city: place.name || city,
            temperature: data.current.temperature_2m,
            code: data.current.weather_code,
            unit: fahrenheit ? "F" : "C",
            days: daily.time.map((iso, i) => ({ day: weekdayOf(iso), max: daily.temperature_2m_max[i], min: daily.temperature_2m_min[i], code: daily.weather_code[i] })),
        };
        problem = "";
    }).catch((error) => {
        if (serial !== fetchSerial) return;
        weather = null;
        problem = "weather unavailable";
        console.error("weather-clock: " + (error && error.message ? error.message : error));
    });
}

/** (Re)start the periodic weather refresh with the current settings. */
function schedule() {
    if (refreshTimer !== null) clearInterval(refreshTimer);
    refreshTimer = setInterval(refresh, wholeSetting(opus.applet.settings.refreshMinutes, DEFAULT_REFRESH_MINUTES) * 60000);
}

opus.ui.show({ type: "canvas", id: "clock", width: WIDTH, height: HEIGHT, draw });
setInterval(() => opus.ui.redraw("clock"), 1000);
opus.applet.onSettings(() => {
    schedule();
    refresh();
});
schedule();
refresh();
