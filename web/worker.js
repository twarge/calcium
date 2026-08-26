// Evaluation lives in this worker so a slow document costs late answers, not
// a frozen page — the same trade the apps made when evaluation left the main
// thread. Requests carry a sequence number; the page discards any reply that
// a newer request has superseded, so replies need no ordering guarantee.
//
// The handler is installed before the engine finishes loading: requests that
// arrive early wait on the same promise and run, in order, once it settles.
import { load } from "./engine.js";

const ready = load();

self.onmessage = async ({ data }) => {
  const { call } = await ready;
  const answers = JSON.parse(call("calcium_evaluate", data.text) ?? "[]");
  postMessage({ seq: data.seq, answers });
};
