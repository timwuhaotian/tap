// What an agent sees of a page and does on it: the page side of the bench's `snapshot`
// and `act` (docs/PLAN.md §8 "Agents use Tap"). Tap's own, not Search's.
//
// The body of an async function. `verb` ("snapshot", "click", "fill", "press"), `ref`
// and `value` arrive as arguments (callAsyncJavaScript), never pasted into the source;
// it runs in Tap's own content world, so the page can't read or forge the refs, and the
// DOM is never marked with them.
const key = "__tapAgentRefs";
const refs = globalThis[key] ?? (globalThis[key] = []);

const CONTROLS = [
    "a[href]", "button", "input:not([type=hidden])", "select", "textarea", "summary",
    "[role=button]", "[role=link]", "[role=checkbox]", "[role=radio]", "[role=switch]",
    "[role=tab]", "[role=menuitem]", "[role=option]", "[role=combobox]", "[role=textbox]",
    "[role=searchbox]", "[contenteditable='']", "[contenteditable=true]", "[onclick]",
].join(",");
const LIMIT = 400;
const clip = (text, length = 80) => {
    const flat = String(text ?? "").replace(/\s+/g, " ").trim();
    return flat.length > length ? flat.slice(0, length - 1) + "…" : flat;
};

function shown(element) {
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    if (element.checkVisibility) return element.checkVisibility({ checkVisibilityCSS: true });
    const style = getComputedStyle(element);
    return style.visibility !== "hidden" && style.display !== "none";
}

function role(element) {
    const given = element.getAttribute("role");
    if (given) return given;
    const tag = element.tagName.toLowerCase();
    if (tag === "a") return "link";
    if (tag === "select") return "combobox";
    if (tag === "textarea") return "textbox";
    if (tag === "summary") return "button";
    if (tag === "input") {
        const type = (element.getAttribute("type") || "text").toLowerCase();
        if (["button", "submit", "reset", "image"].includes(type)) return "button";
        if (type === "checkbox" || type === "radio") return type;
        if (type === "search") return "searchbox";
        if (type === "range") return "slider";
        return "textbox";
    }
    if (element.isContentEditable) return "textbox";
    return tag === "button" ? "button" : "clickable";
}

function name(element) {
    const label = element.getAttribute("aria-label");
    if (label) return clip(label);
    const by = element.getAttribute("aria-labelledby");
    if (by) {
        const text = by.split(/\s+/).map(id => document.getElementById(id)?.innerText ?? "").join(" ");
        if (text.trim()) return clip(text);
    }
    if (element.labels?.length) return clip(element.labels[0].innerText);
    const tag = element.tagName.toLowerCase();
    if (tag === "input" && ["button", "submit", "reset"].includes(element.type)) return clip(element.value);
    // A list's text is every option in it; what it is called is said elsewhere.
    const text = tag === "select" ? "" : element.innerText;
    if (text?.trim()) return clip(text);
    return clip(element.getAttribute("placeholder") || element.getAttribute("title")
        || element.querySelector("img[alt]")?.getAttribute("alt") || element.getAttribute("name") || "");
}

function describe(element, index) {
    const item = { ref: "e" + (index + 1), role: role(element), name: name(element) };
    const tag = element.tagName.toLowerCase();
    if (tag === "a") item.href = element.href;
    if (tag === "input" || tag === "textarea" || tag === "select") {
        if (element.type === "checkbox" || element.type === "radio") item.checked = element.checked;
        else if (element.type === "password") item.value = element.value ? "••••" : "";
        else if (tag === "select") item.value = clip(element.selectedOptions[0]?.text ?? "");
        else if (element.value) item.value = clip(element.value);
    }
    if (element.disabled || element.getAttribute("aria-disabled") === "true") item.disabled = true;
    return item;
}

if (verb === "snapshot") {
    const found = [...document.querySelectorAll(CONTROLS)].filter(shown);
    refs.length = 0;
    refs.push(...found.slice(0, LIMIT));
    return {
        url: location.href,
        title: document.title,
        controls: refs.map(describe),
        more: Math.max(0, found.length - LIMIT),
    };
}

// A key with no ref goes where the page's focus is, as a key press does.
const element = verb === "press" && !ref
    ? (document.activeElement ?? document.body)
    : refs[Number(String(ref ?? "").replace(/^@?e/, "")) - 1];
if (!element || !element.isConnected) {
    return { error: `no control ${ref} on this page now: take a snapshot again` };
}
if (verb === "click" && (element.disabled || element.getAttribute("aria-disabled") === "true")) {
    return { error: `${ref} is disabled` };
}
if (ref) element.scrollIntoView({ block: "center", inline: "center" });

if (verb === "click") {
    const rect = element.getBoundingClientRect();
    const at = { bubbles: true, cancelable: true, composed: true, view: window, button: 0,
        clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2 };
    // After the answer: a click that leaves the page would take the answer with it.
    setTimeout(() => {
        element.dispatchEvent(new PointerEvent("pointerdown", at));
        element.dispatchEvent(new MouseEvent("mousedown", at));
        element.focus?.();
        element.dispatchEvent(new PointerEvent("pointerup", at));
        element.dispatchEvent(new MouseEvent("mouseup", at));
        element.click();
    }, 0);
    const clicked = describe(element, refs.indexOf(element));
    delete clicked.checked; // as it was before the click, which lands after this answer
    return { clicked };
}

if (verb === "fill") {
    const text = String(value ?? "");
    element.focus?.();
    const tag = element.tagName.toLowerCase();
    if (tag === "select") {
        const option = [...element.options].find(o => o.value === text || o.text.trim() === text);
        if (!option) return { error: `no option "${text}"` };
        element.value = option.value;
    } else if (tag === "textarea" || (tag === "input" && !["checkbox", "radio"].includes(element.type))) {
        // The prototype's setter, so a framework that watches the property sees a change.
        const prototype = tag === "input" ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
        Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, text);
        element.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true,
            inputType: "insertText", data: text }));
    } else if (element.isContentEditable) {
        getSelection().selectAllChildren(element);
        if (!document.execCommand("insertText", false, text)) element.textContent = text;
    } else {
        return { error: `${ref} is a ${role(element)}, not something to fill` };
    }
    element.dispatchEvent(new Event("change", { bubbles: true }));
    return { filled: describe(element, refs.indexOf(element)) };
}

if (verb === "press") {
    const pressed = String(value || "Enter");
    const codes = { Enter: 13, Tab: 9, Escape: 27, Backspace: 8, ArrowDown: 40, ArrowUp: 38,
        ArrowLeft: 37, ArrowRight: 39, " ": 32 };
    const keyCode = codes[pressed] ?? pressed.toUpperCase().charCodeAt(0);
    const event = { key: pressed, keyCode, which: keyCode, bubbles: true, cancelable: true, composed: true };
    element.focus?.();
    setTimeout(() => {
        const going = element.dispatchEvent(new KeyboardEvent("keydown", event));
        if (pressed === "Enter" || pressed.length === 1) {
            element.dispatchEvent(new KeyboardEvent("keypress", event));
        }
        element.dispatchEvent(new KeyboardEvent("keyup", event));
        // A dispatched Enter never submits on its own; a real one in a form's field does.
        if (going && pressed === "Enter" && element.form && element.tagName !== "TEXTAREA") {
            element.form.requestSubmit();
        }
    }, 0);
    return { pressed, on: describe(element, refs.indexOf(element)) };
}

return { error: `unknown action: ${verb}` };
