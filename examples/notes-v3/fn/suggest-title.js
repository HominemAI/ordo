// suggestTitle: a short title for a note's text, from a language model.
//
// Declared in ../notes.ordo. `input` holds the declared parameters;
// `ctx.secret("llmKey")` is a handle the host fills in on the way out, only
// for api.anthropic.com. Throwing an Error answers the page `function_error`.

const MODEL = "claude-haiku-4-5";

export default {
  async call(input, ctx) {
    const text = String(input.text || "").slice(0, 4000);
    if (!text.trim()) return { title: "" };

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
    console.log("suggested a title of", title.length, "characters");
    return { title };
  },
};
