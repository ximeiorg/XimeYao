#!/usr/bin/env node
// 文档结构机检。用法：node scripts/doccheck.mjs
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const problems = [];
const fail = (file, msg) => problems.push(`${file}: ${msg}`);

const readText = (rel) => readFileSync(join(ROOT, rel), 'utf8').replace(/\r\n/g, '\n');
const lineCount = (rel) => readText(rel).replace(/\n$/, '').split('\n').length;
const stripCode = (line) => line.replace(/`[^`]*`/g, '');

const RULE_FILES = ['AGENTS.md', 'WORKFLOW.md', 'RULES.md', 'VISUAL.md', 'TEAM.md'];
const LIMITS = {
  'AGENTS.md': 90,
  'WORKFLOW.md': 130,
  'RULES.md': 80,
  'VISUAL.md': 50,
  'TEAM.md': 90,
  'PROGRESS.md': 150,
};
const TABLE_FILES = [...RULE_FILES, 'PROGRESS.md', 'docs/verify/README.md'];

// 1. 规则文件与 PROGRESS 的行数上限
for (const [file, cap] of Object.entries(LIMITS)) {
  if (!existsSync(join(ROOT, file))) {
    fail(file, '文件不存在');
    continue;
  }
  const n = lineCount(file);
  if (n > cap) fail(file, `共 ${n} 行，超过上限 ${cap}`);
}

// 2. PROGRESS 必备区块
const progLines = readText('PROGRESS.md').split('\n');
for (const heading of ['## 接手指南', '## 当前状态', '## 进行中', '## 已完成', '## 未决', '## 验证记录', '## 下一步']) {
  if (!progLines.some((l) => l.trim() === heading)) fail('PROGRESS.md', `缺少区块 ${heading}`);
}

// 3. PROGRESS 销账要整行删，不留 [x]
progLines.forEach((line, i) => {
  if (/\[x\]/i.test(line)) fail('PROGRESS.md', `第 ${i + 1} 行仍有 [x]；销账应整行删除`);
});

// 4. 表格结构：分隔行存在，且每行列数与表头一致
function checkTables(file) {
  const lines = readText(file).split('\n');
  let inFence = false;
  let block = [];
  const flush = () => {
    if (block.length === 0) return;
    const startLine = block[0].i + 1;
    const rows = block.map((b) => b.text.trim());
    if (rows.length < 2) {
      fail(file, `第 ${startLine} 行表格缺少表头分隔行`);
    } else {
      if (!/^\|[\s:|-]+\|$/.test(rows[1])) fail(file, `第 ${startLine} 行表格缺少表头分隔行`);
      const cols = (rows[0].match(/\|/g) || []).length;
      rows.forEach((row, k) => {
        if ((row.match(/\|/g) || []).length !== cols) {
          fail(file, `第 ${block[k].i + 1} 行表格列数与表头不一致`);
        }
      });
    }
    block = [];
  };
  lines.forEach((text, i) => {
    if (/^\s*```/.test(text)) {
      inFence = !inFence;
      flush();
      return;
    }
    if (inFence) return;
    if (/^\s*\|/.test(text)) block.push({ text, i });
    else flush();
  });
  flush();
}
TABLE_FILES.forEach(checkTables);

// 5. DECISIONS 的 D-nnn 条目：字段齐全、状态合规、单条 ≤50 行、编号连续
const decLines = readText('DECISIONS.md').split('\n');
const dEntries = [];
decLines.forEach((line, i) => {
  const m = /^## (D-\d{3})\s/.exec(line);
  if (m) dEntries.push({ id: m[1], start: i, block: [] });
});
const FIELDS = ['日期', '状态', '背景', '决策', '理由', '影响与代价', '遗留'];
dEntries.forEach((entry) => {
  let end = decLines.length;
  for (let j = entry.start + 1; j < decLines.length; j++) {
    if (/^## /.test(decLines[j])) {
      end = j;
      break;
    }
  }
  entry.block = decLines.slice(entry.start, end);
  const body = entry.block.join('\n');
  if (entry.block.length > 50) fail('DECISIONS.md', `${entry.id} 共 ${entry.block.length} 行，超过上限 50`);
  for (const field of FIELDS) {
    if (!new RegExp(`^- ${field}[：:]`, 'm').test(body)) fail('DECISIONS.md', `${entry.id} 缺少字段「${field}」`);
  }
  const status = /^- 状态[：:]\s*(.+)$/m.exec(body);
  if (status && !/^(生效|被 D-\d{3} 取代|作废)/.test(status[1].trim())) {
    fail('DECISIONS.md', `${entry.id} 状态值不合规：${status[1].trim()}`);
  }
});
if (dEntries.length > 0) {
  const nums = dEntries.map((e) => Number(e.id.slice(2))).sort((a, b) => a - b);
  nums.forEach((n, i) => {
    if (n !== i + 1) fail('DECISIONS.md', `D-nnn 编号不连续：期望 D-${String(i + 1).padStart(3, '0')}`);
  });
}

// 6. 禁止行号引用（会漂移）
const LINE_REF = /(?:^|[^\w/])([\w./\\-]+\.(?:rs|md|toml|ya?ml|json|ps1|wxs|wxl)):(\d+)/;
function checkLineRefs(file, lines) {
  lines.forEach((line, i) => {
    if (line.includes('http')) return;
    if (LINE_REF.test(line)) fail(file, `第 ${i + 1} 行出现行号引用：${line.trim().slice(0, 60)}`);
  });
}
RULE_FILES.forEach((f) => checkLineRefs(f, readText(f).split('\n')));
checkLineRefs('PROGRESS.md', progLines);
dEntries.forEach((e) => checkLineRefs('DECISIONS.md', e.block));

// 7. 禁止裸 §（必须带文件名）
function checkBareSection(file, lines) {
  lines.forEach((line, i) => {
    if (/^\s*#/.test(line)) return;
    const bare = stripCode(line);
    if (bare.includes('§') && !/[\w./-]+\.md\s*§/.test(bare)) {
      fail(file, `第 ${i + 1} 行有裸 §，需带文件名`);
    }
  });
}
RULE_FILES.forEach((f) => checkBareSection(f, readText(f).split('\n')));
checkBareSection('PROGRESS.md', progLines);
dEntries.forEach((e) => checkBareSection('DECISIONS.md', e.block));

// 8. 相对链接目标必须存在
const verifyDateFiles = () =>
  readdirSync(join(ROOT, 'docs/verify'))
    .filter((f) => /^\d{4}-\d{2}-\d{2}-.*\.md$/.test(f))
    .map((f) => `docs/verify/${f}`);

function checkLinks(file) {
  const text = readText(file);
  for (const m of text.matchAll(/\[[^\]]*\]\(([^)\s]+)\)/g)) {
    let target = m[1];
    if (/^(https?:|mailto:|#)/.test(target) || target.includes('://')) continue;
    target = target.split('#')[0];
    if (!target) continue;
    const abs = resolve(join(ROOT, dirname(file)), decodeURIComponent(target));
    if (!existsSync(abs)) fail(file, `链接目标不存在：${m[1]}`);
  }
}
[...RULE_FILES, 'PROGRESS.md', 'DECISIONS.md', 'docs/verify/README.md', ...verifyDateFiles()].forEach(checkLinks);

// 9. RULES 单条 ≤200 字
readText('RULES.md').split('\n').forEach((line, i) => {
  if (/^- \[[GJF]\]/.test(line) && line.length > 200) {
    fail('RULES.md', `第 ${i + 1} 行规则 ${line.length} 字，超过 200 字`);
  }
});

// 10. 验证档案必须有四列表头
for (const file of verifyDateFiles()) {
  if (!readText(file).includes('| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |')) {
    fail(file, '缺少证据四列表头');
  }
}

if (problems.length === 0) {
  console.log('doccheck: 通过');
  process.exit(0);
}
console.error('doccheck: 未通过');
for (const p of problems) console.error(`  ✗ ${p}`);
process.exit(1);
