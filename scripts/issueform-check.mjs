#!/usr/bin/env node
// issue 表单结构机检。用法：node scripts/issueform-check.mjs [目录]
//
// 为什么要它：GitHub 的 issue form 是「整份表单一起校验」——任何一个 body 元素不合法，
// 整份表单就会被静默丢弃，New issue 页上根本不会出现这个模板，且仓库里看不到任何报错。
// 2026-11-13 就因此丢过「缺陷反馈（Bug）」表单（logs 字段的 placeholder 写在了
// attributes 外面），而另外两个表单正常，很容易误判成「文件没推上去」。
//
// 这里不引第三方 YAML 依赖（CI 与本机都不装），改为按缩进做结构检查：
// 只认 GitHub 文档里写明的顶层键、body 元素键、attributes 键与 validations 键，
// 多写一个键就报错。等价于把 schema 的 allowed 列表手写一份。
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIR = resolve(ROOT, process.argv[2] || '.github/ISSUE_TEMPLATE');

const problems = [];
const fail = (file, msg) => problems.push(`${file}: ${msg}`);

const TOP_KEYS = new Set([
  'name', 'description', 'body', 'assignees', 'labels', 'title', 'type', 'projects',
]);
const ELEMENT_KEYS = new Set(['type', 'id', 'attributes', 'validations']);
const VALIDATION_KEYS = new Set(['required', 'min_length']);
const ATTR_KEYS = {
  markdown: new Set(['value']),
  textarea: new Set(['label', 'description', 'placeholder', 'value', 'render']),
  input: new Set(['label', 'description', 'placeholder', 'value']),
  dropdown: new Set(['label', 'description', 'multiple', 'options', 'default']),
  checkboxes: new Set(['label', 'description', 'options']),
};
const REQUIRED_ATTRS = {
  markdown: ['value'],
  textarea: ['label'],
  input: ['label'],
  dropdown: ['label', 'options'],
  checkboxes: ['label', 'options'],
};
const ID_RE = /^[A-Za-z0-9_-]+$/;

function readLines(rel) {
  return readFileSync(join(DIR, rel), 'utf8').replace(/\r\n/g, '\n').split('\n');
}

// 顶层键 + 元素键 + attributes/validations 键的结构检查
function checkForm(file) {
  const lines = readLines(file);
  const seenTop = new Set();
  const seenIds = new Set();
  const seenLabels = [];
  let inBody = false;
  let el = null; // 当前 body 元素
  let block = null; // 当前元素内的块：attributes | validations

  const closeEl = () => {
    if (!el) return;
    for (const need of REQUIRED_ATTRS[el.type] || []) {
      if (!el.attrs.has(need)) fail(file, `body 元素 ${el.type}（id=${el.id || '-'}）缺少 attributes.${need}`);
    }
    el = null;
    block = null;
  };

  lines.forEach((raw, i) => {
    const line = raw.replace(/\t/g, '  ');
    if (!line.trim() || line.trim().startsWith('#')) return;
    const indent = line.length - line.trimStart().length;
    const text = line.trim();

    // 顶层键：缩进 0
    if (indent === 0) {
      closeEl();
      inBody = false;
      const m = text.match(/^([A-Za-z_][\w-]*):(.*)$/);
      if (!m) return;
      const key = m[1];
      seenTop.add(key);
      if (!TOP_KEYS.has(key)) {
        fail(file, `第 ${i + 1} 行：顶层键 ${key} 不被 GitHub 支持（允许：${[...TOP_KEYS].join(', ')}）`);
      }
      if (key === 'body') inBody = true;
      return;
    }

    // body 元素起点：缩进 2 的 "- type: xxx"
    if (inBody && indent === 2 && text.startsWith('- ')) {
      closeEl();
      const m = text.slice(2).match(/^type:\s*(\S+)\s*$/);
      if (!m) {
        fail(file, `第 ${i + 1} 行：body 元素缺少 type`);
        el = { type: 'unknown', id: null, attrs: new Set(), line: i + 1 };
        block = null;
        return;
      }
      el = { type: m[1], id: null, attrs: new Set(), line: i + 1 };
      if (!(m[1] in ATTR_KEYS)) {
        fail(file, `第 ${i + 1} 行：type ${m[1]} 不是受支持的元素类型`);
      }
      return;
    }

    if (!el) return;

    // 元素级键：缩进 4
    if (indent === 4) {
      const m = text.match(/^([A-Za-z_][\w-]*):(.*)$/);
      if (!m) return;
      const key = m[1];
      if (!ELEMENT_KEYS.has(key)) {
        fail(file, `第 ${i + 1} 行：body 元素下出现非法键 ${key}（只允许 type / id / attributes / validations）`);
      }
      if (key === 'id') {
        const id = m[2].trim().replace(/^["']|["']$/g, '');
        el.id = id;
        if (!ID_RE.test(id)) fail(file, `第 ${i + 1} 行：id "${id}" 只能含字母、数字、- 和 _`);
        if (seenIds.has(id)) fail(file, `第 ${i + 1} 行：id "${id}" 重复`);
        seenIds.add(id);
      }
      block = key === 'attributes' ? 'attributes' : key === 'validations' ? 'validations' : null;
      return;
    }

    // attributes / validations 内的键：缩进 6
    if (indent === 6 && block) {
      const m = text.match(/^([A-Za-z_][\w-]*):(.*)$/);
      if (!m) return;
      const key = m[1];
      if (block === 'attributes') {
        const allowed = ATTR_KEYS[el.type];
        if (allowed && !allowed.has(key)) {
          fail(file, `第 ${i + 1} 行：${el.type} 的 attributes 下不允许 ${key}（允许：${[...allowed].join(', ')}）`);
        }
        el.attrs.add(key);
        if (key === 'label') {
          const label = m[2].trim().replace(/^["']|["']$/g, '');
          if (label) {
            seenLabels.push({ label, line: i + 1 });
          }
        }
      } else if (!VALIDATION_KEYS.has(key)) {
        fail(file, `第 ${i + 1} 行：validations 下不允许 ${key}（允许：${[...VALIDATION_KEYS].join(', ')}）`);
      }
    }
  });
  closeEl();

  for (const need of ['name', 'description', 'body']) {
    if (!seenTop.has(need)) fail(file, `缺少顶层键 ${need}`);
  }
  // label 相同且都没有 id 时 GitHub 会报「labels are too similar」
  for (let a = 0; a < seenLabels.length; a += 1) {
    for (let b = a + 1; b < seenLabels.length; b += 1) {
      const x = seenLabels[a];
      const y = seenLabels[b];
      if (x.label === y.label) fail(file, `第 ${y.line} 行：label "${x.label}" 与第 ${x.line} 行重复`);
    }
  }
}

function checkConfig(file) {
  const allowed = new Set(['blank_issues_enabled', 'contact_links']);
  readLines(file).forEach((raw, i) => {
    const line = raw.replace(/\t/g, '  ');
    if (!line.trim() || line.trim().startsWith('#')) return;
    const indent = line.length - line.trimStart().length;
    if (indent !== 0) return;
    const m = line.trim().match(/^([A-Za-z_][\w-]*):(.*)$/);
    if (!m) return;
    if (!allowed.has(m[1])) {
      fail(file, `第 ${i + 1} 行：config.yml 顶层键 ${m[1]} 不被支持（允许：${[...allowed].join(', ')}）`);
    }
  });
}

if (!existsSync(DIR)) {
  console.error(`issueform-check: 目录不存在 ${DIR}`);
  process.exit(1);
}

const files = readdirSync(DIR).filter((f) => f.endsWith('.yml') || f.endsWith('.yaml')).sort();
if (files.length === 0) {
  console.error(`issueform-check: ${DIR} 下没有 issue 表单文件`);
  process.exit(1);
}

for (const f of files) {
  if (f === 'config.yml') checkConfig(f);
  else checkForm(f);
}

if (problems.length > 0) {
  console.error(`issueform-check: 失败（${problems.length} 项）`);
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`issueform-check: 通过（${files.length} 个文件）`);
