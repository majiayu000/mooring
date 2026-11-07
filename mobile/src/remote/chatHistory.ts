// Chat cursors are byte offsets within one indexed history, not immutable message IDs.
export type HistoryMessage = { seq: number; at: number; role: string; text: string };
export type HistoryPage<T> = {
  session: string; generation: string; messages: T[]; more: boolean; partial?: boolean;
};

export async function readChatPage<T>(
  fetch: (fields: Record<string, unknown>) => Promise<HistoryPage<T>>,
  fields: Record<string, unknown>,
  generation: string | null,
): Promise<HistoryPage<T> & { reset: boolean }> {
  let page = await fetch(fields);
  const reset = generation !== null && generation !== page.generation;
  if (reset && (fields.after !== undefined || fields.before !== undefined)) {
    // The old cursor may hide a corrected reply or a deletion, including an empty result.
    page = await fetch({ session: page.session, limit: 50, ...(fields.tools ? { tools: true } : {}) });
  }
  return { ...page, reset };
}

export function mergeChat<T extends HistoryMessage>(have: T[], got: T[], reset = false): T[] {
  if (reset) return got;
  const key = (m: T) => `${m.at}|${m.role}|${m.text}`;
  const seen = new Set(have.map(key));
  const fresh = got.filter((m) => !seen.has(key(m)));
  return fresh.length ? [...have, ...fresh].sort((a, b) => a.seq - b.seq) : have;
}
