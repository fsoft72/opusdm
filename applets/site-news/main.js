// Site News: shows the latest items of an RSS 2.0 (or Atom) feed. The feed is
// parsed with regular expressions because the script engine has no XML parser.

/** The most items shown when the setting is missing or invalid. */
const DEFAULT_MAX_ITEMS = 8;
/** The refresh period when the setting is missing or invalid. */
const DEFAULT_REFRESH_MINUTES = 15;
/** The timeout of one feed request. */
const FETCH_TIMEOUT_MS = 15000;

/** The interval timer of the periodic refresh. */
let refreshTimer = null;
/** Counts fetches: an answer of an older fetch is ignored. */
let fetchSerial = 0;

/**
 * Replace the XML entities and numeric references of `text`.
 * @param {string} text
 * @returns {string}
 */
function decodeEntities(text) {
    return text
        .replace(/&#x([0-9a-fA-F]+);/g, (m, hex) => String.fromCodePoint(parseInt(hex, 16)))
        .replace(/&#([0-9]+);/g, (m, dec) => String.fromCodePoint(parseInt(dec, 10)))
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/&quot;/g, "\"")
        .replace(/&apos;/g, "'")
        .replace(/&amp;/g, "&");
}

/**
 * The text of an XML element body: CDATA is kept as is, the rest has its
 * entities decoded.
 * @param {string} raw
 * @returns {string}
 */
function elementText(raw) {
    const cdata = /^\s*<!\[CDATA\[([\s\S]*?)\]\]>\s*$/.exec(raw);
    return (cdata ? cdata[1] : decodeEntities(raw)).trim();
}

/**
 * The text of the first `<tag>` element in `block`, or "".
 * @param {string} block
 * @param {string} tag
 * @returns {string}
 */
function tagText(block, tag) {
    const match = new RegExp("<" + tag + "(?:\\s[^>]*)?>([\\s\\S]*?)</" + tag + ">").exec(block);
    return match ? elementText(match[1]) : "";
}

/**
 * The link of an item: RSS `<link>text</link>`, else an Atom `<link href="..."/>`.
 * @param {string} block
 * @returns {string}
 */
function itemLink(block) {
    const text = tagText(block, "link");
    if (text) return text;
    const atom = /<link\s[^>]*href\s*=\s*["']([^"']+)["'][^>]*>/.exec(block);
    return atom ? decodeEntities(atom[1]).trim() : "";
}

/**
 * Extract the items of a feed. Items without a title or a link are skipped.
 * @param {string} text the feed document
 * @returns {Array<{title: string, link: string}>}
 */
function parseFeed(text) {
    const items = [];
    const pattern = /<(item|entry)(?:\s[^>]*)?>([\s\S]*?)<\/\1>/g;
    let match;
    while ((match = pattern.exec(text)) !== null) {
        const title = tagText(match[2], "title");
        const link = itemLink(match[2]);
        if (title && link) items.push({ title, link });
    }
    return items;
}

/**
 * The title of the feed itself (the text before the first item).
 * @param {string} text the feed document
 * @returns {string}
 */
function channelTitle(text) {
    const first = text.search(/<(item|entry)[\s>]/);
    return tagText(first < 0 ? text : text.slice(0, first), "title");
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
 * Show a window holding only a message.
 * @param {string} message
 */
function showMessage(message) {
    opus.ui.show({ type: "box", orientation: "vertical", spacing: 6, children: [{ type: "label", id: "message", text: message }] });
}

/**
 * Show the title and the list of items.
 * @param {string} title the feed title, may be ""
 * @param {Array<{title: string, link: string}>} items
 */
function showItems(title, items) {
    const children = [];
    if (title) children.push({ type: "label", id: "title", text: title });
    children.push({
        type: "list",
        id: "items",
        items: items.map((item) => ({ text: item.title, onClick: () => opus.applet.open(item.link) })),
    });
    opus.ui.show({ type: "box", orientation: "vertical", spacing: 6, children });
}

/**
 * Report a failure: visible in the window and in the log.
 * @param {string} message
 */
function fail(message) {
    console.error("site-news: " + message);
    showMessage(message);
}

/** Fetch the feed and show it (or the reason it cannot be shown). */
function refresh() {
    const settings = opus.applet.settings;
    const url = String(settings.feedUrl || "").trim();
    if (!url) {
        showMessage("Set the feed URL in the settings of this applet.");
        return;
    }
    const serial = ++fetchSerial;
    opus.http.get(url, { timeoutMs: FETCH_TIMEOUT_MS }).then((reply) => {
        if (serial !== fetchSerial) return;
        if (!reply.ok) return fail("The feed answered HTTP " + reply.status + ".");
        const items = parseFeed(reply.body).slice(0, wholeSetting(settings.maxItems, DEFAULT_MAX_ITEMS));
        if (items.length === 0) return fail("No items found in the feed.");
        showItems(channelTitle(reply.body), items);
    }).catch((error) => {
        if (serial === fetchSerial) fail("Cannot read the feed: " + (error && error.message ? error.message : error));
    });
}

/** (Re)start the periodic refresh with the current settings. */
function schedule() {
    if (refreshTimer !== null) clearInterval(refreshTimer);
    const minutes = wholeSetting(opus.applet.settings.refreshMinutes, DEFAULT_REFRESH_MINUTES);
    refreshTimer = setInterval(refresh, minutes * 60000);
}

opus.applet.onSettings(() => {
    schedule();
    refresh();
});
schedule();
refresh();
