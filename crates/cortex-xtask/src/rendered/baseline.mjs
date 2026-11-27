// Test-only observation of desktop IPC and of actual X11 root pixels.
import { createServer, createConnection } from 'node:net';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, readFileSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const MARKER = 'Native fixture progress.';
const PEER_MARKER = 'Post-question coordinator text';
const MAX_FRAME = 8 * 1024 * 1024;
const SECOND = 1_000_000_000;
const now = () => Number(process.hrtime.bigint());

export function classify({method, params} = {}) {
  const operation = method === 'workspace' || method === 'native.query' ? params?.operation : method;
  if (operation === 'history.page') return 'history';
  if (['tasks.get', 'executions.get', 'decisions.get', 'decisions.list', 'results.get'].includes(operation)
      || method === 'machine.call' && params?.op === 'product.facts') return 'detail';
  // The desktop real port negotiates capabilities on each machine-wide coherent
  // read, not on project-scoped reads. Do not count reconnect's projects.list.
  if (method === 'service.capabilities') return 'fullRefresh';
  return undefined;
}

export function isSubscription(method) {
  return method === 'service.subscribe' || method === 'service.subscribe.code';
}

export function markerIn(value) {
  if (!value || typeof value !== 'object') return false;
  if (typeof value.text === 'string' && value.text.startsWith(MARKER)) return true;
  return Object.values(value).some(markerIn);
}

export function exactHistoryRow(page) {
  const rows = page?.items?.filter(row => row.kind === 'assistant'
    && (row.text === PEER_MARKER || row.text === '\n\n' + PEER_MARKER));
  if (rows?.length > 1) throw new Error('ambiguous fixture history fragment');
  return rows?.[0];
}

export function parsePeerTrace(text) {
  const match = /^([1-9]\d{0,9}) (fixture-[1-9]\d{0,7}) (\d{1,16}) (\d{1,16})\n$/.exec(text);
  if (!match) throw new Error('missing, duplicate or malformed peer emission record');
  const [, pid, sessionId, first, last] = match;
  const [begin, end] = [Number(first), Number(last)];
  if (![begin, end].every(Number.isSafeInteger) || begin <= 0 || end < begin) {
    throw new Error('invalid peer emission clock');
  }
  return {pid: Number(pid), sessionId, begin, end};
}

export function bindPeer(emission, bindings, scope, start, history) {
  const [binding] = Array.isArray(bindings) ? bindings : [];
  if (bindings?.length !== 1 || !binding) {
    throw new Error('peer emission lacks one matching durable fixture binding; rows=' + (bindings?.length ?? 'invalid'));
  }
  for (const [field, matches] of [
    ['task', binding.taskId === scope.taskId], ['project', binding.projectId === scope.projectId],
    ['session', binding.sessionId === emission.sessionId],
    ['runtime task', /^t_[a-f0-9]{32}$/.test(binding.runtimeTaskId)],
    ['attempt', /^xa_[a-f0-9]{32}$/.test(binding.attemptId)],
    ['display session', typeof binding.displaySessionId === 'string' && binding.displaySessionId],
    ['source sequence', Number.isSafeInteger(binding.sourceSequence) && binding.sourceSequence >= 0],
  ]) {
    if (!matches) throw new Error('peer emission lacks matching fixture ' + field);
  }
  const rowId = 'wh_' + createHash('sha256').update(JSON.stringify(
    [binding.displaySessionId, binding.sourceSequence])).digest('hex').slice(0, 32);
  if (scope.historyId !== rowId || scope.historySequence !== String(binding.sourceSequence + 1)) {
    throw new Error('peer fragment is not the same durable history row');
  }
  if (!(start < emission.begin && emission.end <= history)) {
    throw new Error('peer emission outside the measured clock window');
  }
  return binding;
}

export function rateSummary(events, start, end) {
  const size = Math.floor((end - start) / SECOND);
  if (size < 1 || size > 120) throw new Error('incomplete request observation window');
  const totals = { history: 0, detail: 0, fullRefresh: 0 };
  const windows = Array.from({length: size}, () => ({...totals}));
  for (const {time, kind} of events) {
    if (time < start || time >= end) continue;
    totals[kind]++;
    const bin = windows[Math.floor((time - start) / SECOND)];
    if (bin) bin[kind]++;
  }
  return {durationNs: end - start, totals, windows};
}

export function correlate(arrival, frames, arrivalEnd = arrival) {
  if (!Number.isSafeInteger(arrival)) throw new Error('missing text-bearing history reply');
  let previous, seenBefore = false;
  for (const frame of frames) {
    if (frame.begin > frame.end || previous && previous.end > frame.begin) {
      throw new Error('unordered frame capture');
    }
    if (frame.visible) {
      if (!previous || frame.begin < arrivalEnd) throw new Error('unbracketed first visible frame');
      if (!seenBefore) throw new Error('no pre-arrival negative frame');
      return {
        latencyUpperNs: frame.end - arrival,
        latencyLowerNs: Math.max(0, previous.begin - arrivalEnd),
        frameGapNs: frame.end - previous.begin,
      };
    }
    seenBefore ||= frame.end < arrival;
    previous = frame;
  }
  throw new Error('no captured frame contains the fragment');
}

// Forward bytes unchanged, and independently decode complete frames. The
// observation timestamp is the arrival of the last byte in this read callback.
export function decoder(callback) {
  let pending = Buffer.alloc(0);
  return {
    push(chunk, time) {
      pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
      while (pending.length >= 4) {
        const len = pending.readUInt32BE(0);
        if (len < 1 || len > MAX_FRAME) throw new Error('invalid observation frame length');
        if (pending.length < len + 4) break;
        let message;
        try { message = JSON.parse(pending.toString('utf8', 4, len + 4)); }
        catch { throw new Error('invalid observation frame JSON'); }
        callback(message, time);
        pending = pending.subarray(len + 4);
      }
    },
    complete() { return pending.length === 0; },
  };
}

function ocr(header, image, peerTiming) {
  return new Promise((resolve, reject) => {
    const cmd = spawn('tesseract', ['stdin', 'stdout', '--psm', '11', '-l', 'eng'], {
      stdio: ['pipe', 'pipe', 'ignore'],
      env: {...process.env, OMP_THREAD_LIMIT: '1'},
    });
    let output = '';
    cmd.on('error', reject);
    cmd.stdin.on('error', reject);
    cmd.stdout.on('data', data => { output += data; });
    const deadline = setTimeout(() => cmd.kill('SIGKILL'), 5000);
    cmd.on('close', code => {
      clearTimeout(deadline);
      if (code !== 0) reject(new Error('pixel OCR failed'));
      else resolve((peerTiming ? /\bpost[- ]question coordinator text\b/i : /\bnative fixture progress\b/i)
        .test(output.replace(/\s+/g, ' ')));
    });
    cmd.stdin.end(Buffer.concat([header, image]));
  });
}

function capture(window, arrival, ready, fail, peerTiming) {
  const spawnTime = now();
  const proc = spawn('/opt/cortex-render/baseline-frame', [window], {stdio: ['ignore', 'pipe', 'ignore']});
  let pending = Buffer.alloc(0), header, length, size, complete = false, resolving;
  const frames = [];
  let count = 0, first, last, maxGap = 0, maxCapture = 0, maxReadLag = 0;
  proc.on('error', () => fail('frame helper did not start'));
  proc.stdout.on('data', chunk => {
    if (complete) return;
    const received = now();
    pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
    if (!header) {
      const split = pending.indexOf(10);
      if (split < 0 && pending.length < 24) return;
      const match = /^(\d+) (\d+)$/.exec(pending.toString('ascii', 0, split));
      if (!match || +match[1] > 1920 || +match[2] > 1200 || +match[1] * +match[2] < 1) {
        return fail('invalid frame geometry');
      }
      size = [+match[1], +match[2]];
      length = size[0] * size[1];
      header = Buffer.from('P5\n' + size.join(' ') + '\n255\n');
      pending = pending.subarray(split + 1);
    }
    while (pending.length >= 16 + length) {
      // The helper and Node are Linux processes in the same container/time
      // namespace. Check clock ordering using the parent spawn/pipe brackets.
      const begin = Number(pending.readBigUInt64LE(0));
      const end = Number(pending.readBigUInt64LE(8));
      if (!Number.isSafeInteger(end) || begin < spawnTime || begin < (last?.end ?? 0)
          || end < begin || end > received) return fail('capture clock alignment failed');
      if (!arrival()) {
        if (frames.length === 6) frames.shift();
      }
      if ((!arrival() || begin <= arrival() + 2 * SECOND) && frames.length < 100) {
        frames.push({begin, end, image: Buffer.from(pending.subarray(16, 16 + length))});
      } else if (arrival() && begin <= arrival() + 2 * SECOND) {
        return fail('frame retention overflow');
      }
      first ??= begin;
      maxGap = Math.max(maxGap, last ? begin - last.begin : 0);
      maxCapture = Math.max(maxCapture, end - begin);
      maxReadLag = Math.max(maxReadLag, received - end);
      last = {begin, end};
      pending = pending.subarray(16 + length);
      if (++count === 1) ready();
    }
  });
  const done = new Promise(resolve => { resolving = resolve; });
  proc.on('close', () => {
    if (!complete) fail('frame capture ended early');
    resolving();
  });
  return {
    async stop() { complete = true; proc.kill(); await done; },
    async analyze(time, timeEnd = time) {
      const matches = [];
      let previous;
      for (const frame of frames) {
        const visible = previous?.image.equals(frame.image) ? previous.visible : await ocr(header, frame.image, peerTiming);
        previous = {image: frame.image, visible};
        matches.push({begin: frame.begin, end: frame.end, visible});
        if (visible) break;
      }
      const firstVisible = matches.at(-1);
      const previousNegative = matches.at(-2);
      return {...correlate(time, matches, timeEnd), ...(peerTiming ? {firstVisible, previousNegative} : {}),
        frameCount: count, frameWidth: size[0], frameHeight: size[1],
        firstFrameBeginNs: first, maxGapNs: maxGap, maxCaptureNs: maxCapture, maxReadLagNs: maxReadLag};
    },
  };
}

async function observe(dir, backend, front, peerTrace) {
  const readTrace = () => {
    const size = statSync(peerTrace).size;
    if (!size) return undefined;
    if (size > 128) throw new Error('peer trace overflow or duplicate emission');
    return parsePeerTrace(readFileSync(peerTrace, 'ascii'));
  };
  let liveEmission;
  const publish = (name, value) => {
    writeFileSync(join(dir, name + '.tmp'), JSON.stringify(value), {mode: 0o600});
    renameSync(join(dir, name + '.tmp'), join(dir, name));
  };
  const fail = reason => {
    if (!failure) { failure = reason; publish('failure', reason); }
  };
  let failure, start, end, sampler, arrival, scope, expectedTask;
  const warmupStart = now();
  let subscriptions = 0, warmupRefreshes = 0, activeRequests = 0, connections = 0;
  const events = [], clients = new Set(), warmupMethods = new Set();
  let subscribeRequests = 0, subscribeReplies = 0, subscribeClosed = 0;
  const server = createServer(client => {
    if (clients.size >= 256) { fail('too many observed connections'); client.destroy(); return; }
    connections++;
    const service = createConnection(backend);
    clients.add(client);
    let request, subscribed = false, req, res;
    req = decoder((message, time) => {
      if (request || message.jsonrpc !== '2.0' || typeof message.method !== 'string' || !message.id) {
        throw new Error('unsupported request envelope');
      }
      request = message;
      if (start === undefined && /^service\.[A-Za-z.]+$/.test(message.method)) warmupMethods.add(message.method);
      if (isSubscription(message.method)) subscribeRequests++;
      const kind = classify(message);
      if (start === undefined) {
        if (kind === 'fullRefresh') warmupRefreshes++;
      } else if (end === undefined) {
        if (++activeRequests > 8192) throw new Error('request observation overflow');
        if (kind) events.push({kind, time});
      }
    });
    res = decoder((message, time) => {
      if (isSubscription(request?.method) && message.id === request.id) {
        subscribeReplies++;
        if (message.result?.subscribed) {
          subscribed = true;
          subscriptions++;
        }
      }
      if (end === undefined && request && classify(request) === 'history'
          && message.id === request.id && (peerTrace || markerIn(message.result))) {
        const page = request.method === 'native.query'
          ? (message.result?.projectId === request.params.projectId && message.result?.data?.type === 'history'
            ? message.result.data.value : undefined)
          : message.result;
        const row = peerTrace && exactHistoryRow(page);
        if (peerTrace && !row) return;
        if (start === undefined) throw new Error('text marker already arrived before measurement');
        const p = request.params;
        const taskId = p.payload?.target ?? p.target;
        if (taskId === expectedTask && !arrival) {
          scope = {projectId: p.projectId, taskId, rpcId: request.id};
          if (!scope.projectId) throw new Error('missing history correlation identity');
          if (row) {
            if (!row.historyId || !row.sequence) throw new Error('missing fixture history row identity');
            scope.historyId = row.historyId;
            scope.historySequence = row.sequence;
          }
          arrival = time;
        }
      }
    });
    // Pipe supplies backpressure and preserves half-close and payloads. The
    // extra listeners observe exactly those chunks, not a second connection.
    for (const [socket, decode] of [[client, req], [service, res]]) {
      socket.on('data', chunk => {
        const time = now();
        try { decode.push(chunk, time); } catch (error) { fail(error.message); }
      });
      socket.on('end', () => { if (!decode.complete()) fail('partial IPC frame'); });
      socket.on('error', error => {
        if (end === undefined) fail('observed IPC socket failure: ' + (socket === service ? 'backend ' : 'desktop ') + error.code);
        client.destroy();
        service.destroy();
      });
    }
    client.on('close', () => {
      clients.delete(client);
      if (subscribed) {
        subscribeClosed++;
        subscriptions--;
        if (start !== undefined && end === undefined) fail('subscription lost during window');
      }
      service.destroy();
    });
    service.on('close', () => client.end());
    client.pipe(service);
    service.pipe(client);
  });
  server.on('error', () => fail('observer socket failed'));
  server.listen(front, () => {
    try {
      // The real desktop connector refuses sockets with group/other access.
      chmodSync(front, 0o600);
      publish('ready', true);
    } catch { fail('observer could not make its socket private'); }
  });
  try {
    while (!existsSync(join(dir, 'finish')) && !failure) {
      if (!sampler && existsSync(join(dir, 'begin'))) {
        if (subscriptions < 1 || warmupRefreshes < 1) {
          throw new Error('missing desktop IPC warmup: subscriptions=' + subscriptions
            + ', refreshes=' + warmupRefreshes + ', connections=' + connections
            + ', subscribe sent/reply/closed=' + [subscribeRequests, subscribeReplies, subscribeClosed].join('/')
            + ', methods=' + [...warmupMethods].sort().join(','));
        }
        const {window, taskId} = JSON.parse(readFileSync(join(dir, 'begin'), 'utf8'));
        if (!/^\d+$/.test(window) || !/^wt_[0-9a-f]{32}$/.test(taskId)) throw new Error('missing owned window or task identity');
        expectedTask = taskId;
        sampler = capture(window, () => {
          if (!peerTrace) return arrival;
          try { liveEmission ??= readTrace(); }
          catch (error) { fail(error.message); }
          return liveEmission?.end;
        }, () => {
          start = now();
          publish('started', true);
        }, fail, Boolean(peerTrace));
      }
      await new Promise(resolve => setTimeout(resolve, 5));
    }
    end = now();
    await sampler?.stop();
    if (failure) throw new Error(failure);
    if (!sampler || !start || subscriptions < 1 || activeRequests === 0 || !arrival) {
      throw new Error('incomplete observation: subscriptions=' + subscriptions + ', requests='
        + activeRequests + ', history=' + events.filter(event => event.kind === 'history').length
        + ', fragment=' + Boolean(arrival));
    }
    const emission = peerTrace && readTrace();
    if (peerTrace && !emission) throw new Error('no complete fixture ACP emission was observed');
    const peerTiming = emission ? {
      emission,
      fragment: PEER_MARKER,
      binding: bindPeer(emission, JSON.parse(readFileSync(join(dir, 'finish'), 'utf8')), scope, start, arrival),
      historyArrivalNs: arrival,
      isolation: 'loopback-only network, masked checkout stores, authenticated private Xvfb',
    } : undefined;
    const summary = emission
      ? await sampler.analyze(emission.begin, emission.end)
      : await sampler.analyze(arrival);
    publish('result', {...summary, ...rateSummary(events, start, end), scope, peerTiming,
      arrivalNsFromWindowStart: arrival - start, warmupNs: start - warmupStart,
      warmupRefreshes, subscriptions, activeRequests, connections});
    // Rust stops this observer only after it closes the measured desktop window.
    await new Promise(resolve => process.once('SIGTERM', resolve));
  } finally {
    await sampler?.stop();
    for (const connection of clients) connection.destroy();
    server.close();
  }
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const [dir, backend, front, peerTrace] = process.argv.slice(2);
  observe(dir, backend, front, peerTrace).catch(error => {
    writeFileSync(join(dir, 'failure'), JSON.stringify(error.message), {mode: 0o600});
    process.exitCode = 1;
  });
}
