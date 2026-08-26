// Loads the WebAssembly engine and returns the string-in, string-out caller.
// Shared by the page (tokens and line kinds, the every-keystroke paint) and
// the worker (evaluation); each side instantiates its own copy of the same
// cached module, so a long evaluation never blocks the paint.
export async function load() {
  const wasm = await WebAssembly.instantiateStreaming(
    fetch(new URL("calcium_ffi.wasm", import.meta.url)));
  const engine = wasm.instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  /** Calls a `char* -> char*` export, owning both buffers correctly. */
  function call(name, text) {
    const bytes = encoder.encode(text);
    const inPtr = engine.calcium_alloc(bytes.length + 1);
    new Uint8Array(engine.memory.buffer, inPtr, bytes.length).set(bytes);
    new Uint8Array(engine.memory.buffer)[inPtr + bytes.length] = 0;
    const outPtr = engine[name](inPtr);
    engine.calcium_dealloc(inPtr, bytes.length + 1);
    if (!outPtr) return null;
    // Re-acquire the buffer: it may have moved if memory grew during the call.
    const memory = new Uint8Array(engine.memory.buffer);
    let end = outPtr;
    while (memory[end] !== 0) end++;
    const result = decoder.decode(memory.subarray(outPtr, end));
    engine.calcium_string_free(outPtr);
    return result;
  }

  return { call };
}
