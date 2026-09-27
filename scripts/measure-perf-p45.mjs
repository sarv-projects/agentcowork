import { performance } from 'node:perf_hooks';
import { writeFileSync, readFileSync, unlinkSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

console.error(
  'This script does not measure SQLite, DNS, or the sidecar. The numbers it used to print were file reads and an in-memory Map. Run `cargo test -p agentcowork-storage --lib synchronous_normal_and_mmap -- --nocapture` for the database timings.',
)
process.exit(2)

console.log('=== AgentCowork P45 Performance Queue Benchmark ===\n');

const results = {
  timestamp: new Date().toISOString(),
  environment: {
    node: process.version,
    platform: process.platform,
    arch: process.arch,
  },
  metrics: {},
};

console.log('Measuring P45.1 & P45.2: Storage & Read Latency...');
const tempDir = join(process.cwd(), '.perf_scratch');
if (!existsSync(tempDir)) mkdirSync(tempDir, { recursive: true });

const testFile = join(tempDir, 'perf_sample.dat');
const sampleBuffer = Buffer.alloc(10 * 1024 * 1024, 0xab);
writeFileSync(testFile, sampleBuffer);

const directReadStart = performance.now();
for (let i = 0; i < 50; i++) {
  const read = readFileSync(testFile);
  if (read.length !== sampleBuffer.length) throw new Error('Read mismatch');
}
const directReadDuration = performance.now() - directReadStart;
const avgReadMs = directReadDuration / 50;

results.metrics.p45_1_sqlite_pragmas = {
  journal_mode: 'WAL',
  synchronous: 'NORMAL',
  cache_size_kb: 64000,
  mmap_size_mb: 256,
  status: 'verified',
};

results.metrics.p45_2_read_latency = {
  sample_size_mb: 10,
  iterations: 50,
  total_duration_ms: Math.round(directReadDuration * 100) / 100,
  avg_read_latency_ms: Math.round(avgReadMs * 100) / 100,
  throughput_mb_sec: Math.round(((10 * 50) / (directReadDuration / 1000)) * 100) / 100,
};

console.log('Measuring P45.3: Write Burst Throughput...');
const burstStart = performance.now();
const burstFile = join(tempDir, 'burst.log');
const writes = [];
for (let i = 0; i < 1000; i++) {
  writes.push(JSON.stringify({ seq: i, ts: Date.now(), event: 'state_checkpoint', status: 'ok' }) + '\n');
}
writeFileSync(burstFile, writes.join(''));
const burstDuration = performance.now() - burstStart;

results.metrics.p45_3_wal_burst = {
  writes_count: 1000,
  duration_ms: Math.round(burstDuration * 100) / 100,
  ops_per_sec: Math.round((1000 / (burstDuration / 1000)) * 100) / 100,
};

console.log('Measuring P45.5: Audit Batching Throughput...');
const auditStart = performance.now();
const batchBuffer = [];
for (let i = 0; i < 5000; i++) {
  const hash = createHash('sha256').update('event_' + i).digest('hex');
  batchBuffer.push(JSON.stringify({ seq: i, hash, type: 'turn_boundary' }));
}
const auditRaw = batchBuffer.join('\n');
const auditDuration = performance.now() - auditStart;

results.metrics.p45_5_audit_batching = {
  records_processed: 5000,
  batch_hash_duration_ms: Math.round(auditDuration * 100) / 100,
  throughput_events_sec: Math.round((5000 / (auditDuration / 1000)) * 100) / 100,
};

console.log('Measuring P45.7: Route/DNS Cache Latency...');
const dnsCache = new Map();
dnsCache.set('api.anthropic.com', '104.18.2.14');
dnsCache.set('api.openai.com', '104.18.7.192');
dnsCache.set('api.x.ai', '172.67.132.88');

const dnsStart = performance.now();
for (let i = 0; i < 100000; i++) {
  const hit = dnsCache.get('api.anthropic.com');
  if (!hit) throw new Error('DNS cache miss');
}
const dnsDuration = performance.now() - dnsStart;

results.metrics.p45_7_dns_cache = {
  lookups: 100000,
  total_duration_ms: Math.round(dnsDuration * 100) / 100,
  lookup_ns_per_op: Math.round(((dnsDuration * 1e6) / 100000) * 100) / 100,
};

const mem = process.memoryUsage();
results.metrics.p45_8_idle_footprint = {
  rss_mb: Math.round((mem.rss / (1024 * 1024)) * 100) / 100,
  heap_used_mb: Math.round((mem.heapUsed / (1024 * 1024)) * 100) / 100,
  heap_total_mb: Math.round((mem.heapTotal / (1024 * 1024)) * 100) / 100,
};

console.log('Measuring P45.9: JSON Streaming Parsing Speed...');
const largePayload = JSON.stringify({
  session_id: 'perf-s1',
  tools: Array.from({ length: 500 }, (_, i) => ({
    id: 'tool_' + i,
    name: 'Tool ' + i,
    schema: { type: 'object', properties: { q: { type: 'string' } } },
  })),
  history: Array.from({ length: 200 }, (_, i) => ({
    role: i % 2 === 0 ? 'user' : 'assistant',
    content: 'Message block ' + i + ' with code snippets and markdown content',
  })),
});

const jsonStart = performance.now();
for (let i = 0; i < 200; i++) {
  const parsed = JSON.parse(largePayload);
  if (!parsed.session_id) throw new Error('JSON parse failed');
}
const jsonDuration = performance.now() - jsonStart;

results.metrics.p45_9_json_throughput = {
  payload_size_kb: Math.round((largePayload.length / 1024) * 100) / 100,
  iterations: 200,
  duration_ms: Math.round(jsonDuration * 100) / 100,
  mb_parsed_per_sec: Math.round((((largePayload.length * 200) / (1024 * 1024)) / (jsonDuration / 1000)) * 100) / 100,
};

try {
  unlinkSync(testFile);
  unlinkSync(burstFile);
} catch {}

console.log('\n=== Benchmark Results ===');
console.log(JSON.stringify(results, null, 2));

const outPath = join(process.cwd(), 'scripts', 'p45-live-measurements.json');
writeFileSync(outPath, JSON.stringify(results, null, 2), 'utf-8');
console.log('\nResults written to ' + outPath);
console.log('P45 Benchmark Suite completed successfully.');
