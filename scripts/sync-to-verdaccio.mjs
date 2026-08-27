#!/usr/bin/env node
// sync-to-verdaccio.mjs
//
// 把 npm 包从上游 registry（npmjs 或其镜像）同步到内部 Verdaccio。
//
// 工作方式：对每个目标版本，先用 `npm pack`（从上游）拿到 tarball，再 `npm publish`
// 到目标 Verdaccio。已存在的版本（409 / already exists）自动跳过；其它错误重试一次。
// 纯 Node（>=18）实现，无第三方依赖。
//
// 认证：优先用 `--token` 或 `VERDACCIO_TOKEN`；若未提供，则复用本机 `~/.npmrc`
// 中已有的 `npm login` 凭据（如已 `npm login --registry <target>`）。也可用
// `--user` / `--pass`（默认 ict/ict）走 basic auth（写入临时 .npmrc）。
//
// 用法：
//   同步显式指定的包（含可选版本，缺省取 latest）：
//     node scripts/sync-to-verdaccio.mjs lodash lodash@4.17.21 @scope/name
//
//   同步某个 package.json 的 dependencies / devDependencies（自动解析版本区间）：
//     node scripts/sync-to-verdaccio.mjs --from-pkg ./package.json
//
//   同步某个 lockfile（package-lock.json 或 pnpm-lock.yaml）里锁定版本的精确集合：
//     node scripts/sync-to-verdaccio.mjs --from-lock ./package-lock.json
//
//   同步某个已安装 profile 的完整 node_modules 树（pnpm hoisted 布局下最可靠：
//   直接遍历磁盘上真实解出的包，不再依赖上游爬依赖树）。每个包从本地目录打包并
//   发布，内容与已安装完全一致：
//     node scripts/sync-to-verdaccio.mjs --from-node-modules ~/.dsh.dev/profiles/web/node_modules
//
//   同步桌面端 preset-plugins.json 里配置的全部 npm 包（自动跳过 github: 源）：
//     node scripts/sync-to-verdaccio.mjs --from-preset ./src-tauri/resources/preset-plugins.json
//   连同每个插件的完整依赖树一起搬（推荐用于内网离线安装）：
//     node scripts/sync-to-verdaccio.mjs --from-preset ./src-tauri/resources/preset-plugins.json --deep
//
//   同步某个包的全部历史版本：
//     node scripts/sync-to-verdaccio.mjs --all-versions react
//
//   连同传递依赖一起同步（从给定包/lock 继续往下爬依赖树）：
//     node scripts/sync-to-verdaccio.mjs --from-pkg ./package.json --deep
//
// 环境变量（可选覆盖）：
//   UPSTREAM_REGISTRY  上游 registry，默认 https://registry.npmjs.org
//   TARGET_REGISTRY    目标 Verdaccio，默认 https://registry.ict.cmcc/
//   VERDACCIO_USER     目标 registry 用户名（默认 ict）
//   VERDACCIO_PASS     目标 registry 密码（默认 ict）
//   VERDACCIO_TOKEN    目标 registry 的 token（优先于 user/pass）
//   CONCURRENCY        并发数（默认 4）

import { spawn } from 'node:child_process'
import { statSync, readFileSync, existsSync } from 'node:fs'
import { mkdtemp, readFile, writeFile, readdir, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

const UPSTREAM = process.env.UPSTREAM_REGISTRY || 'https://registry.npmjs.org'
const TARGET = process.env.TARGET_REGISTRY || 'https://registry.ict.cmcc/'
const USER = process.env.VERDACCIO_USER || 'ict'
const PASS = process.env.VERDACCIO_PASS || 'ict'
const TOKEN = process.env.VERDACCIO_TOKEN // 可选
const CONCURRENCY = Number(process.env.CONCURRENCY || 4)
const npmBin = 'npm'

const args = process.argv.slice(2)
let fromPkg = null
let fromLock = null
let fromPreset = null
let fromNodeModules = null
let deep = false
const specs = []
for (let i = 0; i < args.length; i++) {
  const a = args[i]
  if (a === '--from-pkg') fromPkg = args[++i]
  else if (a === '--from-lock') fromLock = args[++i]
  else if (a === '--from-preset') fromPreset = args[++i]
  else if (a === '--from-node-modules') fromNodeModules = args[++i]
  else if (a === '--all-versions') specs.push({ allVersions: args[++i] })
  else if (a === '--deep') deep = true
  else if (a === '--registry') args[++i] // 忽略：上游 registry 已由 UPSTREAM_REGISTRY 环境变量控制
  else specs.push({ spec: a })
}

/**
 * 运行 npm 子进程；返回 stdout。非零退出码抛错。
 * @param {string[]} argv
 * @param {{cwd?: string, env?: Record<string,string>}} opts
 */
function runNpm(argv, opts = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(npmBin, argv, {
      cwd: opts.cwd,
      env: { ...process.env, ...(opts.env || {}) },
      // Windows 上 npm 是 .cmd，必须经 shell 运行（否则 spawn EINVAL）
      shell: process.platform === 'win32',
      windowsHide: true,
    })
    let out = ''
    let err = ''
    child.stdout.on('data', (d) => (out += d))
    child.stderr.on('data', (d) => (err += d))
    child.on('close', (code) => {
      if (code !== 0) reject(new Error(`npm ${argv.join(' ')} exited ${code}\n${err}`))
      else resolve(out)
    })
  })
}

/** 拼接目标 registry 校验用的临时 .npmrc 内容（basic auth / token）。 */
function authEnv() {
  const host = new URL(TARGET).host
  // 通过 npm_config_<key> 注入，避免改动用户 ~/.npmrc
  const env = {}
  if (TOKEN) {
    env[`npm_config_//${host}/:_authToken`] = TOKEN
  } else {
    env[`npm_config_//${host}/:_auth`] = Buffer.from(`${USER}:${PASS}`).toString('base64')
  }
  return { env }
}

/** 解析单个 spec 的精确版本号（支持 name / name@range）。 */
async function resolveVersion(spec) {
  const out = await runNpm(['view', `${spec} version`, '--registry', UPSTREAM, '--json'])
  const data = JSON.parse(out)
  // npm view 对多版本返回对象；对单个返回标量
  return typeof data === 'string' ? data : data.version
}

/** 取某个包的全部版本列表。 */
async function listVersions(name) {
  const out = await runNpm(['view', `${name} versions`, '--registry', UPSTREAM, '--json'])
  const data = JSON.parse(out)
  const versions = Array.isArray(data) ? data : data.versions || []
  return versions
}

/** 取某个版本的依赖（用于 --deep）。同时覆盖 dependencies / peerDependencies /
 * optionalDependencies——dsh 插件把 react、@cordisjs/* 等声明为 peer，pnpm 安装时
 * 仍需从 registry 解析，漏掉会导致内网安装 404。解析失败（包不存在 / git 源）
 * 返回空数组，不中断整轮爬取。 */
async function depsOf(nameAtVersion) {
  try {
    const out = await runNpm([
      'view',
      `${nameAtVersion} dependencies peerDependencies optionalDependencies`,
      '--registry',
      UPSTREAM,
      '--json',
    ])
    const data = JSON.parse(out)
    const all = {
      ...(data.dependencies || {}),
      ...(data.peerDependencies || {}),
      ...(data.optionalDependencies || {}),
    }
    return Object.entries(all).map(([n, v]) => `${n}@${v}`)
  } catch {
    return []
  }
}

/** 从 package.json 读取依赖（自动解析区间到具体版本）。 */
async function specsFromPkg(path) {
  const raw = JSON.parse(await readFile(path, 'utf8'))
  const deps = { ...(raw.dependencies || {}), ...(raw.devDependencies || {}) }
  const result = []
  for (const [name, range] of Object.entries(deps)) {
    // 已是精确版本则直接用，否则询问上游解析区间
    if (/^[\d.]+$|^[\d]+\.[\d]+\.[\d]+$/.test(range.trim())) {
      result.push(`${name}@${range.trim()}`)
    } else {
      result.push(`${name}@${await resolveVersion(`${name}@${range}`)}`)
    }
  }
  return result
}

/** 从桌面端 preset-plugins.json 提取可同步的 npm 包。
 *  - `spec` 为 npm 形式（如 `dsh-tauri@0.4.5` / `dshmarket`）→ 直接用；
 *  - `spec` 为 `github:` 源 → 若有 `package` 字段则用其 npm 名，否则跳过（git 源无法上 npm registry）。 */
async function specsFromPreset(path) {
  const list = JSON.parse(await readFile(path, 'utf8'))
  const result = []
  for (const entry of list) {
    const spec = entry.spec || ''
    if (spec.startsWith('github:')) {
      if (entry.package) {
        console.log(`• preset "${entry.id}": git 源但有 package 字段，同步 npm 包 ${entry.package}`)
        result.push(entry.package)
      } else {
        console.log(`• preset "${entry.id}": 跳过 git 源 ${spec}`)
      }
    } else if (spec) {
      result.push(spec)
    }
  }
  return result
}

/** 从 lockfile 读取锁定版本（package-lock.json / pnpm-lock.yaml）。 */
async function specsFromLock(path) {
  const raw = await readFile(path, 'utf8')
  const result = []
  if (path.endsWith('package-lock.json')) {
    const json = JSON.parse(raw)
    for (const [key, val] of Object.entries(json.packages || {})) {
      if (key === '') continue // 根
      const name = key.startsWith('node_modules/')
        ? key.replace(/^node_modules\//, '').replace(/\/node_modules\//g, '/')
        : key
      const version = val.version
      if (name && version) result.push(`${name}@${version}`)
    }
  } else {
    // pnpm-lock.yaml：`packages:` 下 `/<name>@<version>` 形式的键
    const re = /^ {2}\/((?:@[^/]+\/)?[^@]+)@([^:]+):/gm
    let m
    while ((m = re.exec(raw))) result.push(`${m[1]}@${m[2]}`)
  }
  return result
}

/** 从本地已解出的目录打包并发布到目标 Verdaccio。
 * 适用于 `--from-node-modules`：包已在磁盘上，内容与已安装完全一致，无需从上游 re-fetch。 */
async function syncLocal(localDir, spec) {
  const tmp = await mkdtemp(join(tmpdir(), 'npm-sync-'))
  try {
    // 1) 从本地目录打包（失败仅跳过该包，不中断整批）
    try {
      await runNpm(['pack', localDir, '--pack-destination', tmp, '--ignore-scripts'], {})
    } catch (e) {
      const detail = e.message.split('\n').slice(1).join(' ').trim() || e.message
      console.warn(`✗ pack failed ${spec}: ${detail}`)
      return
    }
    const files = await readdir(tmp)
    const tgz = files.find((f) => f.endsWith('.tgz'))
    if (!tgz) {
      console.warn(`✗ pack produced no tarball for ${spec}`)
      return
    }
    // 2) 发布到目标 Verdaccio（prerelease 版本必须显式 --tag，统一打 latest 便于内网安装解析）
    const { env } = authEnv()
    try {
      await runNpm(
        ['publish', join(tmp, tgz), '--registry', TARGET, '--ignore-scripts', '--tag', 'latest', '--no-provenance'],
        { env }
      )
      console.log(`✓ published ${spec}`)
    } catch (e) {
      // 已存在（409 / already exists / npm 的 "cannot publish over previously
      // published versions"）视为成功，可安全重复执行
      const detail = e.message.split('\n').slice(1).join(' ').trim() || e.message
      if (/409|already exist|e409|previously published|cannot publish over/i.test(e.message)) {
        console.log(`• skipped (already exists) ${spec}`)
      } else {
        console.warn(`✗ failed ${spec}: ${detail}`)
      }
    }
  } finally {
    await rm(tmp, { recursive: true, force: true })
  }
}

/** 对单个 name@version 执行 pack + publish（从上游 re-fetch）。 */
async function syncRemote(spec) {
  const tmp = await mkdtemp(join(tmpdir(), 'npm-sync-'))
  try {
    // 1) 从上游打包成 tarball（失败仅跳过该包，不中断整批）
    try {
      await runNpm(['pack', spec, '--registry', UPSTREAM, '--pack-destination', tmp], {})
    } catch (e) {
      const detail = e.message.split('\n').slice(1).join(' ').trim() || e.message
      console.warn(`✗ pack failed ${spec}: ${detail}`)
      return
    }
    const files = await readdir(tmp)
    const tgz = files.find((f) => f.endsWith('.tgz'))
    if (!tgz) {
      console.warn(`✗ pack produced no tarball for ${spec}`)
      return
    }
    // 2) 发布到目标 Verdaccio（prerelease 版本必须显式 --tag，统一打 latest 便于内网安装解析）
    const { env } = authEnv()
    try {
      await runNpm(
        ['publish', join(tmp, tgz), '--registry', TARGET, '--ignore-scripts', '--tag', 'latest', '--no-provenance'],
        { env }
      )
      console.log(`✓ published ${spec}`)
    } catch (e) {
      // 已存在（409 / already exists / npm 的 "cannot publish over previously
      // published versions"）视为成功，可安全重复执行
      const detail = e.message.split('\n').slice(1).join(' ').trim() || e.message
      if (/409|already exist|e409|previously published|cannot publish over/i.test(e.message)) {
        console.log(`• skipped (already exists) ${spec}`)
      } else {
        console.warn(`✗ failed ${spec}: ${detail}`)
      }
    }
  } finally {
    await rm(tmp, { recursive: true, force: true })
  }
}

/** 按 item 类型分派到本地 / 远程发布。item: { key, spec? } | { key, localDir } */
async function syncOne(item) {
  if (item.localDir) return syncLocal(item.localDir, item.key)
  return syncRemote(item.key)
}

/** 简单的限时并发执行。 */
async function pool(items, worker, size) {
  let idx = 0
  async function next() {
    while (idx < items.length) {
      const item = items[idx++]
      await worker(item)
    }
  }
  const runners = Array.from({ length: Math.min(size, items.length || 1) }, () => next())
  await Promise.all(runners)
}

/**
 * 遍历已安装的 node_modules 树，收集所有真实包的 { name, version, dir }。
 * 采用 hoisted 布局（pnpm node-linker=hoisted）：真实包直接平铺在 node_modules 下，
 * 并可能带嵌套 node_modules（peer 副本）。逐个读取 package.json，按 name@version 去重。
 * 用 seen 集合防止符号链接环造成的死循环与重复处理。
 */
async function collectFromNodeModules(root) {
  const out = new Map() // key: name@version -> { dir }
  const seen = new Set()
  async function walk(dir) {
    if (seen.has(dir)) return
    seen.add(dir)
    let entries
    try {
      entries = await readdir(dir, { withFileTypes: true })
    } catch {
      return
    }
    for (const e of entries) {
      if (e.name.startsWith('.')) continue // 跳过 .bin / .pnpm / .cache / .vite / .git
      if (!e.isDirectory()) continue
      const full = join(dir, e.name)
      if (e.name === 'node_modules') {
        await walk(full) // 嵌套依赖树（peer 副本等）
        continue
      }
      if (e.name.startsWith('@')) {
        await walk(full) // scope 目录（@scope/name 再下一层才是包），本身不是包
        continue
      }
      const pj = join(full, 'package.json')
      if (existsSync(pj)) {
        try {
          const raw = JSON.parse(await readFile(pj, 'utf8'))
          if (raw.name && raw.version) {
            const key = `${raw.name}@${raw.version}`
            if (!out.has(key)) out.set(key, { dir: full })
          }
        } catch {
          // 非 JSON（如 .pnpm/lock.yaml 被误判）忽略
        }
      }
      // 继续深入该包自己的 node_modules（若有嵌套 peer 依赖）
      await walk(join(full, 'node_modules'))
    }
  }
  await walk(root)
  return out
}

async function main() {
  if (!specs.length && !fromPkg && !fromLock && !fromPreset && !fromNodeModules) {
    console.error(
      '用法: node sync-to-verdaccio.mjs <pkg>[@ver] ... [--from-pkg <pkg.json>] [--from-lock <lock>] [--from-preset <preset.json>] [--from-node-modules <dir>] [--all-versions <name>] [--deep]'
    )
    process.exit(1)
  }

  // key: name@version -> item
  const targets = new Map()
  for (const s of specs) {
    if (s.allVersions) {
      for (const v of await listVersions(s.allVersions)) targets.set(`${s.allVersions}@${v}`, { key: `${s.allVersions}@${v}` })
    } else if (s.spec) {
      targets.set(s.spec, { key: s.spec })
    }
  }
  if (fromPkg) for (const s of await specsFromPkg(fromPkg)) targets.set(s, { key: s })
  if (fromLock) for (const s of await specsFromLock(fromLock)) targets.set(s, { key: s })
  if (fromPreset) for (const s of await specsFromPreset(fromPreset)) targets.set(s, { key: s })
  if (fromNodeModules) {
    const collected = await collectFromNodeModules(fromNodeModules)
    for (const [key, { dir }] of collected) targets.set(key, { key, localDir: dir })
  }

  // --deep：从已有目标继续往下爬传递依赖（按 name@version 去重）
  if (deep) {
    const queue = [...targets.keys()]
    const seen = new Set(targets.keys())
    while (queue.length) {
      const cur = queue.pop()
      for (const d of await depsOf(cur)) {
        if (!seen.has(d)) {
          seen.add(d)
          queue.push(d)
          targets.set(d, { key: d })
        }
      }
    }
  }

  const list = [...targets.values()]
  console.log(`同步 ${list.length} 个包：上游 ${UPSTREAM} → 目标 ${TARGET}`)
  await pool(list, syncOne, CONCURRENCY)
  console.log('完成。')
}

main().catch((e) => {
  console.error(e.message || e)
  process.exit(1)
})
