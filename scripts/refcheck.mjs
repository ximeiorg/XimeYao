#!/usr/bin/env node
// 引用锚点机检：文件 §章节、D-nnn。用法：node scripts/refcheck.mjs
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const problems = [];
const readText = (rel) => readFileSync(join(ROOT, rel), 'utf8').replace(/\r\n/g, '\n');
const exists = (rel) => existsSync(join(ROOT, rel));

const dateFiles = readdirSync(join(ROOT, 'docs/verify'))
  .filter((f) => /^\d{4}-\d{2}-\d{2}-.*\.md$/.test(f))
  .map((f) => `docs/verify/${f}`);

const FILES = [
  'AGENTS.md',
  'WORKFLOW.md',
  'RULES.md',
  'VISUAL.md',
  'TEAM.md',
  'PROGRESS.md',
  'DECISIONS.md',
  'docs/verify/README.md',
  ...dateFiles,
];

const anchorCache = new Map();
function anchorsOf(file) {
  if (!anchorCache.has(file)) {
    const set = new Set();
    for (const line of readText(file).split('\n')) {
      const m = /^#{1,6}\s*§\s*(\d+(?:\.\d+)?)/.exec(line);
      if (m) set.add(m[1]);
    }
    anchorCache.set(file, set);
  }
  return anchorCache.get(file);
}

const decisions = new Set(
  [...readText('DECISIONS.md').matchAll(/^## (D-\d{3})\s/gm)].map((m) => m[1]),
);

for (const file of FILES) {
  if (!exists(file)) {
    problems.push(`${file}: 文件不存在`);
    continue;
  }
  readText(file).split('\n').forEach((line, idx) => {
    const where = `${file}:${idx + 1}`;
    for (const m of line.matchAll(/([A-Za-z0-9_./-]+\.md)\s*§\s*(\d+(?:\.\d+)?)/g)) {
      const raw = m[0];
      const target = m[1];
      const anchor = m[2];
      if (!exists(target)) {
        problems.push(`${where} 引用文件不存在：${raw}`);
        continue;
      }
      if (!anchorsOf(target).has(anchor)) problems.push(`${where} 锚点不存在：${raw}`);
    }
    for (const m of line.matchAll(/\bD-\d{3}\b/g)) {
      if (!decisions.has(m[0])) problems.push(`${where} 决策不存在：${m[0]}`);
    }
  });
}

if (problems.length === 0) {
  console.log(`refcheck: 通过（${FILES.length} 个文件）`);
  process.exit(0);
}
console.error('refcheck: 未通过');
for (const p of problems) console.error(`  ✗ ${p}`);
process.exit(1);
