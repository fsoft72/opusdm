// Fatto Tasks: the open tasks (every column but DONE) of one board of a Fatto
// instance, read only. It talks to the MCP endpoint of Fatto, the only route an
// API key opens: `POST <baseUrl>/api/mcp-kanban` with JSON-RPC 2.0 `tools/call`.
// Tools used: kanban_workspace_list, kanban_board_list, kanban_board_get_full
// (board, columns and cards in one call) and kanban_card_get (the detail).
//
// Fields used (seen on a real instance):
//   workspace { id, name }            board { id, id_workspace, name }
//   column { id, name, position }     DONE is the column with the highest position
//   card { id, id_board, id_column, code, title, description, position,
//          due_date (ms), done_at, is_archived, is_deleted,
//          assignees, tags: [{ name, slug }], checklists: [{ title, items: [{ text, done }] }] }

/** The endpoint path under the Fatto URL. */
const ENDPOINT = "/api/mcp-kanban";
/** The timeout of one request. */
const REQUEST_TIMEOUT_MS = 20000;
/** The default Fatto URL when the setting is empty. */
const DEFAULT_BASE_URL = "https://fatto.os3.services";
/** The most characters of a description shown in the detail window. */
const MAX_DESCRIPTION_CHARS = 3000;
/** Width and height of the detail window. */
const DETAIL_WIDTH = 460;
const DETAIL_HEIGHT = 420;

/** Workspaces, boards and the open rows now shown. */
let workspaces = [];
let boards = [];
let rows = [];
/** The selected workspace and board indexes (-1: none). */
let wsIndex = -1;
let boardIndex = -1;
/** Counters: an answer of an older request is ignored. */
let wsSerial = 0;
let boardSerial = 0;
let rpcId = 0;
/** The detail windows open now, by card id. */
const detailWindows = new Map();

/**
 * The Fatto URL without trailing slashes.
 * @returns {string}
 */
function baseUrl() {
    const url = String(opus.applet.settings.baseUrl || "").trim() || DEFAULT_BASE_URL;
    return url.replace(/\/+$/, "");
}

/**
 * The text of `error` without ever containing the API key.
 * @param {*} error
 * @returns {string}
 */
function errorText(error) {
    const text = error && error.message ? error.message : String(error);
    const key = String(opus.applet.settings.apiKey || "").trim();
    return key ? text.split(key).join("***") : text;
}

/**
 * Call the tool `name` with `args` on the MCP endpoint and resolve with the
 * tool's JSON answer. Rejects with a readable Error on a network error, an
 * HTTP error, a JSON-RPC error or a tool error.
 * @param {string} name
 * @param {Object} args
 * @returns {Promise<*>}
 */
function callTool(name, args) {
    const key = String(opus.applet.settings.apiKey || "").trim();
    if (!key) return Promise.reject(new Error("Set the API key in the settings of this applet."));
    const body = JSON.stringify({ jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name, arguments: args } });
    const headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Authorization": "Bearer " + key,
    };
    return opus.http.post(baseUrl() + ENDPOINT, { headers, body, timeoutMs: REQUEST_TIMEOUT_MS }).then((reply) => parseReply(name, reply));
}

/**
 * Turn an HTTP reply of the MCP endpoint into the tool's answer.
 * @param {string} name the tool, for messages
 * @param {{status: number, ok: boolean, body: string}} reply
 * @returns {*}
 */
function parseReply(name, reply) {
    if (reply.status === 401) throw new Error("The API key was refused (HTTP 401).");
    let json;
    try {
        json = JSON.parse(reply.body);
    } catch (e) {
        throw new Error(name + ": HTTP " + reply.status + ", the answer is not JSON.");
    }
    if (json.error) throw new Error(name + ": " + (json.error.message || "JSON-RPC error " + json.error.code));
    if (!reply.ok) throw new Error(name + ": HTTP " + reply.status + ".");
    const result = json.result || {};
    const content = Array.isArray(result.content) && result.content[0] ? result.content[0].text : "null";
    let data;
    try {
        data = JSON.parse(content);
    } catch (e) {
        throw new Error(name + ": the tool answer is not JSON.");
    }
    if (result.isError) throw new Error(name + ": " + (data && data.message ? data.message : "the tool failed (status " + (data && data.status) + ")."));
    return data;
}

/**
 * The open cards of a board with the name of their column, in board order:
 * not archived, not deleted, not in DONE (the column with the highest position).
 * @param {Array<Object>} columns
 * @param {Array<Object>} cards
 * @returns {Array<{card: Object, column: string}>}
 */
function openCards(columns, cards) {
    const ordered = (columns || []).slice().sort((a, b) => a.position - b.position);
    if (ordered.length === 0) return [];
    const doneId = ordered[ordered.length - 1].id;
    const byId = new Map(ordered.map((c, index) => [c.id, { name: c.name, index }]));
    return (cards || [])
        .filter((card) => card.id_column !== doneId && !card.is_archived && !card.is_deleted && !card.done_at && byId.has(card.id_column))
        .map((card) => ({ card, column: byId.get(card.id_column).name, order: byId.get(card.id_column).index }))
        .sort((a, b) => a.order - b.order || (a.card.position || 0) - (b.card.position || 0))
        .map(({ card, column }) => ({ card, column }));
}

/**
 * The label of a list row.
 * @param {{card: Object, column: string}} row
 * @returns {string}
 */
function rowText(row) {
    return (row.card.code ? row.card.code + "  " : "") + row.card.title + "  [" + row.column + "]";
}

/**
 * A date from fatto (milliseconds) as YYYY-MM-DD, or "".
 * @param {*} value
 * @returns {string}
 */
function dateText(value) {
    const ms = Number(value);
    if (!Number.isFinite(ms) || ms <= 0) return "";
    return new Date(ms).toISOString().slice(0, 10);
}

/**
 * The names of a list of users or tags: strings, or objects with `name`,
 * `email`, `username` or `id`.
 * @param {*} list
 * @returns {string}
 */
function namesText(list) {
    if (!Array.isArray(list)) return "";
    return list.map((item) => (item && typeof item === "object") ? (item.name || item.username || item.email || item.id_user || item.id || "") : String(item)).filter(Boolean).join(", ");
}

/**
 * The checklists of a card as lines of text.
 * @param {*} checklists
 * @returns {string}
 */
function checklistsText(checklists) {
    if (!Array.isArray(checklists)) return "";
    return checklists.map((list) => {
        const items = Array.isArray(list.items) ? list.items : [];
        const lines = items.map((item) => (item.done ? "[x] " : "[ ] ") + item.text);
        return (list.title || "Checklist") + "\n" + lines.join("\n");
    }).join("\n\n");
}

/**
 * The description cut to MAX_DESCRIPTION_CHARS.
 * @param {*} text
 * @returns {string}
 */
function descriptionText(text) {
    const full = String(text || "");
    return full.length > MAX_DESCRIPTION_CHARS ? full.slice(0, MAX_DESCRIPTION_CHARS) + "\n[...]" : full;
}

/**
 * One "Name: value" label of the detail window, or nothing when the value is empty.
 * @param {string} name
 * @param {string} value
 * @returns {Array<Object>}
 */
function field(name, value) {
    return value ? [{ type: "label", text: name + ": " + value }] : [];
}

/**
 * The widget tree of the detail window of `card`.
 * @param {Object} card the answer of kanban_card_get
 * @param {string} column the name of its column
 * @param {function} onOpen called by the "Open in Fatto" button
 * @returns {Object}
 */
function detailTree(card, column, onOpen) {
    const description = descriptionText(card.description);
    const checklists = checklistsText(card.checklists);
    return { type: "box", orientation: "vertical", spacing: 6, children: [
        { type: "label", id: "title", text: (card.code ? card.code + "  " : "") + card.title, class: "title-3" },
        ...field("Column", column),
        ...field("Assignees", namesText(card.assignees)),
        ...field("Due", dateText(card.due_date)),
        ...field("Tags", namesText(card.tags)),
        ...(description ? [{ type: "label", id: "description", text: description }] : []),
        ...(checklists ? [{ type: "label", id: "checklists", text: checklists }] : []),
        { type: "button", id: "open", label: "Open in Fatto", onClick: onOpen },
    ] };
}

/**
 * Show `message` in the status line.
 * @param {string} message "" hides it
 */
function setStatus(message) {
    opus.ui.update("status", { text: message, visible: message !== "" });
}

/**
 * Report a failure: the log and the status line (never the API key).
 * @param {string} context
 * @param {*} error
 */
function fail(context, error) {
    const message = errorText(error);
    console.error("fatto-tasks: " + context + ": " + message);
    setStatus(message);
}

/** The id of the selected workspace, or "". */
function currentWorkspace() {
    return wsIndex >= 0 && workspaces[wsIndex] ? workspaces[wsIndex].id : "";
}

/** The selected board, or null. */
function currentBoard() {
    return boardIndex >= 0 && boards[boardIndex] ? boards[boardIndex] : null;
}

/**
 * The index of the element of `list` with id `id`, else 0 (the first), else -1.
 * @param {Array<Object>} list
 * @param {string} id
 * @returns {number}
 */
function indexOfId(list, id) {
    const found = list.findIndex((item) => item.id === id);
    return found >= 0 ? found : (list.length > 0 ? 0 : -1);
}

/** Load the workspaces and their boards, keeping the selection when it still exists. */
function loadWorkspaces() {
    const keepBoard = currentBoard() ? currentBoard().id : "";
    const keepWorkspace = currentWorkspace();
    const serial = ++wsSerial;
    boardSerial++;
    setStatus("Loading...");
    callTool("kanban_workspace_list", {}).then((list) => {
        if (serial !== wsSerial) return;
        workspaces = Array.isArray(list) ? list : [];
        wsIndex = indexOfId(workspaces, keepWorkspace);
        opus.ui.update("workspaces", { items: workspaces.map((w) => w.name), selected: wsIndex >= 0 ? wsIndex : null });
        if (wsIndex < 0) return clearBoards("No workspace found.");
        loadBoards(keepBoard);
    }).catch((error) => {
        if (serial === wsSerial) fail("workspaces", error);
    });
}

/**
 * Empty the board dropdown and the list, with `message` in the status line.
 * @param {string} message
 */
function clearBoards(message) {
    boards = [];
    boardIndex = -1;
    rows = [];
    opus.ui.update("boards", { items: [], selected: null });
    opus.ui.update("list", { items: [] });
    setStatus(message);
}

/**
 * Load the boards of the selected workspace, select `keep` (else the first), load its tasks.
 * @param {string} [keep] the id of the board to keep selected
 */
function loadBoards(keep) {
    const serial = ++boardSerial;
    setStatus("Loading...");
    callTool("kanban_board_list", { id_workspace: currentWorkspace(), is_archived: false }).then((list) => {
        if (serial !== boardSerial) return;
        boards = Array.isArray(list) ? list : [];
        boardIndex = indexOfId(boards, keep || "");
        opus.ui.update("boards", { items: boards.map((b) => b.name), selected: boardIndex >= 0 ? boardIndex : null });
        if (boardIndex < 0) return clearBoards("This workspace has no boards.");
        loadTasks();
    }).catch((error) => {
        if (serial === boardSerial) fail("boards", error);
    });
}

/** Load the cards of the selected board and show the open ones. */
function loadTasks() {
    const board = currentBoard();
    if (!board) return;
    const serial = ++boardSerial;
    setStatus("Loading...");
    callTool("kanban_board_get_full", { id_board: board.id }).then((full) => {
        if (serial !== boardSerial) return;
        rows = openCards(full && full.columns, full && full.cards);
        opus.ui.update("list", { items: rows.map((row) => ({ text: rowText(row), onClick: () => openDetail(row) })) });
        setStatus(rows.length === 0 ? "No open tasks on this board." : "");
    }).catch((error) => {
        if (serial === boardSerial) fail("tasks", error);
    });
}

/**
 * Open the detail window of `row`, loading the full card first. A card whose
 * window is open is not opened again.
 * @param {{card: Object, column: string}} row
 */
function openDetail(row) {
    const cardId = row.card.id;
    if (detailWindows.has(cardId)) return;
    callTool("kanban_card_get", { id_card: cardId }).then((card) => {
        if (detailWindows.has(cardId)) return;
        const url = baseUrl() + "/kanban/" + encodeURIComponent(card.id_board || row.card.id_board);
        const handle = opus.ui.openWindow({
            title: (card.code || "Task") + " - " + card.title,
            width: DETAIL_WIDTH,
            height: DETAIL_HEIGHT,
            tree: detailTree(card, row.column, () => opus.applet.open(url)),
            onClose: () => detailWindows.delete(cardId),
        });
        detailWindows.set(cardId, handle);
    }).catch((error) => fail("task " + (row.card.code || cardId), error));
}

/**
 * The user picked workspace `index`.
 * @param {number} index
 */
function onWorkspace(index) {
    if (index === wsIndex) return;
    wsIndex = index;
    loadBoards();
}

/**
 * The user picked board `index`.
 * @param {number} index
 */
function onBoard(index) {
    if (index === boardIndex) return;
    boardIndex = index;
    loadTasks();
}

opus.ui.show({ type: "box", orientation: "vertical", spacing: 6, children: [
    { type: "box", orientation: "horizontal", spacing: 6, children: [
        { type: "dropdown", id: "workspaces", items: [], onChange: onWorkspace },
        { type: "dropdown", id: "boards", items: [], onChange: onBoard },
        { type: "button", id: "refresh", label: "Refresh", onClick: loadWorkspaces },
    ] },
    { type: "label", id: "status", text: "", visible: false },
    { type: "list", id: "list", items: [] },
] });

opus.applet.onSettings(() => loadWorkspaces());
loadWorkspaces();
