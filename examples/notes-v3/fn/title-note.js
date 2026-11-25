// titleNote: give a saved note a title from a language model, and save it.
//
// Declared in ../notes.ordo. `ctx.op` calls an op the declaration lists in
// `uses`, as the person who pressed the button: `oneNote` reads the note
// only if they may read it, and `retitleNote` writes only if they may
// write it. A refused op throws an error whose `code` says why.

const MODEL = "claude-haiku-4-5";

export default {
  async call(input, ctx) {
    const [note] = await ctx.op("oneNote", { id: input.note });
    if (!note) throw new Error("no such note, or it is not yours to read");
    const text = String(note.body || note.title || "").slice(0, 4000);
    if (!text.trim()) return { id: note.id, title: note.title, changed: false };

    const r = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "x-api-key": ctx.secret("llmKey"),
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: MODEL,
        max_tokens: 40,
        messages: [
          {
            role: "user",
            content:
              "Suggest a title of at most 6 words for this note. " +
              "Answer with the title only.\n\n" + text,
          },
        ],
      }),
    });
    if (!r.ok) throw new Error("the model answered " + r.status);

    const answer = await r.json();
    const part = (answer.content || []).find((c) => c.type === "text");
    const title = part ? part.text.trim().replace(/^"|"$/g, "") : "";
    if (!title || title === note.title) {
      return { id: note.id, title: note.title, changed: false };
    }
    await ctx.op("retitleNote", { id: note.id, rev: note.rev, title });
    return { id: note.id, title, changed: true };
  },
};
