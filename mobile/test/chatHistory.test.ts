import { expect, test } from 'bun:test';
import { mergeChat, readChatPage, type HistoryPage, type HistoryMessage } from '../src/remote/chatHistory';

const message = (seq: number, text: string): HistoryMessage => ({ seq, text, at: seq, role: 'agent' });
const page = (generation: string, messages: HistoryMessage[]): HistoryPage<HistoryMessage> =>
  ({ session: 's', generation, messages, more: false });

test('an append keeps older loaded pages and needs one request', async () => {
  let calls = 0;
  const result = await readChatPage(async () => { calls++; return page('a', [message(2, 'new')]); }, { after: 1 }, 'a');
  expect(calls).toBe(1);
  expect(mergeChat([message(1, 'old')], result.messages, result.reset).map(m => m.text)).toEqual(['old', 'new']);
});

for (const cursor of [{ after: 100 }, { before: 100 }]) {
  for (const replacement of [[message(1, 'corrected')], []]) {
    test(`rewrite clears old messages even when delta is empty: ${JSON.stringify(cursor)} / ${replacement.length}`, async () => {
      const calls: Record<string, unknown>[] = [];
      const result = await readChatPage(async fields => {
        calls.push(fields);
        return calls.length === 1 ? page('b', []) : page('b', replacement);
      }, { session: 's', ...cursor, tools: true }, 'a');
      expect(calls).toEqual([{ session: 's', ...cursor, tools: true }, { session: 's', limit: 50, tools: true }]);
      expect(result.reset).toBe(true);
      expect(mergeChat([message(100, 'withdrawn')], result.messages, result.reset)).toEqual(replacement);
    });
  }
}

test('reconnect already has the newest page, including another rewrite during reload', async () => {
  let calls = 0;
  const first = await readChatPage(async () => { calls++; return page('b', []); }, { terminal: 't' }, 'a');
  expect(first.reset).toBe(true);
  expect(calls).toBe(1);
  const again = await readChatPage(async () => page(++calls === 2 ? 'b' : 'c', []), { after: 10 }, 'a');
  expect(again.generation).toBe('c');
  expect(again.reset).toBe(true);
});
