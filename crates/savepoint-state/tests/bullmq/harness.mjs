import { Queue, Worker } from 'bullmq';
import IORedis from 'ioredis';
import fs from 'node:fs';

const args = process.argv.slice(2);
const command = args[0];
const redisUrl = process.env.SP_TEST_BULLMQ_REDIS_URL || 'redis://127.0.0.1:56379/5';
const connection = new IORedis(redisUrl, { maxRetriesPerRequest: null });

async function main() {
  if (command === 'add') {
    const queueName = args[1];
    const jobName = args[2];
    const data = JSON.parse(args[3] || '{}');
    const delay = parseInt(args[4] || '0', 10);
    const q = new Queue(queueName, { connection });
    const job = await q.add(jobName, data, { delay });
    console.log(JSON.stringify({ id: job.id, name: job.name }));
    await q.close();
  } else if (command === 'counts') {
    const queueName = args[1];
    const q = new Queue(queueName, { connection });
    const counts = await q.getJobCounts('waiting', 'active', 'completed', 'failed', 'delayed');
    console.log(JSON.stringify(counts));
    await q.close();
  } else if (command === 'drain') {
    const queueName = args[1];
    const outputFile = args[2];
    const timeoutMs = parseInt(args[3] || '3000', 10);
    const results = [];

    const worker = new Worker(queueName, async (job) => {
      results.push({ id: job.id, name: job.name, data: job.data });
      return { status: 'ok' };
    }, { connection });

    await new Promise((resolve) => setTimeout(resolve, timeoutMs));
    await worker.close();
    fs.writeFileSync(outputFile, JSON.stringify(results));
    console.log(JSON.stringify({ processed: results.length }));
  }
  await connection.quit();
  process.exit(0);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
