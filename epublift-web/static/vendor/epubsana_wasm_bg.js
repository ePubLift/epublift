/**
 * A repair session over one EPUB, held in WASM memory across calls.
 */
export class Session {
    static __wrap(ptr) {
        const obj = Object.create(Session.prototype);
        obj.__wbg_ptr = ptr;
        SessionFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SessionFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_session_free(ptr, 0);
    }
    /**
     * Load an EPUB from its raw bytes, detect its defects, and plan the fixes.
     * @param {Uint8Array} bytes
     * @returns {Session}
     */
    static load(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.session_load(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Session.__wrap(ret[0]);
    }
    /**
     * The session's plan: the starting counts and every proposed fix (each with
     * its current `outcome`). Cheap — it re-validates nothing.
     * @returns {Plan}
     */
    plan() {
        const ret = wasm.session_plan(this.__wbg_ptr);
        return ret;
    }
    /**
     * Run the repair with the fixes at `approved` (0-based [`Fix::index`]
     * values) and return the result — **the core's `epubsana::repair`, the
     * function the CLI runs**, so the same book with the same approvals comes
     * back identical from both.
     *
     * Every approved fix is applied, the book is validated, and a fix that
     * made any finding more frequent is undone and reported `"reverted"`,
     * except for findings a fix revealed (see [`Session::revealed`]). A fix not
     * in `approved` is `"skipped"`, as it is under the CLI's `--apply`.
     *
     * Starts from the book as loaded every time, so calling it again with a
     * different selection replaces the previous run. An index out of range is
     * an error and changes nothing.
     * @param {Uint32Array} approved
     * @param {string | null} [goal]
     * @returns {Report}
     */
    repair(approved, goal) {
        const ptr0 = passArray32ToWasm0(approved, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(goal) ? 0 : passStringToWasm0(goal, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.session_repair(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * **Re-validate** the current EPUB with epubveri and return the result in
     * the shared envelope shape — the same independent check the CLI reports.
     * `goal` is `"valid"` (the default: no fatals and no errors remain) or
     * `"openable"` (no fatals remain — the book opens). Before any
     * [`Session::repair`] every item is `"proposed"` and the book is unchanged.
     * @param {string | null} [goal]
     * @returns {Report}
     */
    report(goal) {
        var ptr0 = isLikeNone(goal) ? 0 : passStringToWasm0(goal, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        const ret = wasm.session_report(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * The repaired EPUB's bytes — download these as `<name>_fixed.epub`.
     * @returns {Uint8Array}
     */
    result_bytes() {
        const ret = wasm.session_result_bytes(this.__wbg_ptr);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * How many findings the last [`Session::repair`] accepted as **revealed**:
     * always in the book, and visible only once a fix let the validator in —
     * by making an unreadable document readable, or by correcting a package
     * version no validator recognises. They were not in the plan, so running
     * again on [`Session::result_bytes`] can repair some of them. `0` before
     * any repair.
     *
     * Not in the envelope's `summary`, which the conventions own; the CLI
     * prints the same number as a note after its report.
     * @returns {number}
     */
    revealed() {
        const ret = wasm.session_revealed(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) Session.prototype[Symbol.dispose] = Session.prototype.free;

/**
 * The version string — the same one the CLI's `-V` and the json envelope's
 * `tool_version` print, git build metadata included.
 * @returns {string}
 */
export function version() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.version();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}
export function __wbg_Error_92b29b0548f8b746(arg0, arg1) {
    const ret = Error(getStringFromWasm0(arg0, arg1));
    return ret;
}
export function __wbg_String_8564e559799eccda(arg0, arg1) {
    const ret = String(arg1);
    const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
    getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
}
export function __wbg___wbindgen_throw_344f42d3211c4765(arg0, arg1) {
    throw new Error(getStringFromWasm0(arg0, arg1));
}
export function __wbg_new_32b398fb48b6d94a() {
    const ret = new Array();
    return ret;
}
export function __wbg_new_da52cf8fe3429cb2() {
    const ret = new Object();
    return ret;
}
export function __wbg_set_6be42768c690e380(arg0, arg1, arg2) {
    arg0[arg1] = arg2;
}
export function __wbg_set_8a16b38e4805b298(arg0, arg1, arg2) {
    arg0[arg1 >>> 0] = arg2;
}
export function __wbindgen_cast_0000000000000001(arg0) {
    // Cast intrinsic for `F64 -> Externref`.
    const ret = arg0;
    return ret;
}
export function __wbindgen_cast_0000000000000002(arg0, arg1) {
    // Cast intrinsic for `Ref(String) -> Externref`.
    const ret = getStringFromWasm0(arg0, arg1);
    return ret;
}
export function __wbindgen_cast_0000000000000003(arg0) {
    // Cast intrinsic for `U64 -> Externref`.
    const ret = BigInt.asUintN(64, arg0);
    return ret;
}
export function __wbindgen_init_externref_table() {
    const table = wasm.__wbindgen_externrefs;
    const offset = table.grow(4);
    table.set(0, undefined);
    table.set(offset + 0, undefined);
    table.set(offset + 1, null);
    table.set(offset + 2, true);
    table.set(offset + 3, false);
}
const SessionFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_session_free(ptr, 1));

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getUint32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;


let wasm;
export function __wbg_set_wasm(val) {
    wasm = val;
}
