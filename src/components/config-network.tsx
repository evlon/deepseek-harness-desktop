import { Description, Input, Label, ListBox, Select, Spinner, Surface } from '@heroui/react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { invoke } from '@tauri-apps/api/core'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { If } from 'react-if-lite'
import { toast } from '@/utils/toast'

/**
 * 网络加速配置（npm 源 + GitHub 镜像）：供「下载插件」界面选择拉包走哪个源。
 *
 * - npm 源：auto（按地域）/ 官方 / npmmirror / 自定义（内网 Verdaccio）
 * - GitHub 镜像：auto / 直连 / ghfast.top / ghproxy / 自定义（内网 git 镜像）
 * - **自动保存**：下拉切换立即保存；自定义 URL 输入停顿后防抖保存（避免
 *   打字过程频繁请求 / 保存到未输完的地址）。保存写回 `update_accel_config`
 *   （落库 + 同步 profile .npmrc），下次安装生效。
 */

export interface AccelConfig {
  npm_registry_mode: string
  npm_registry_custom: string
  npm_registry_url: string
  gh_accel_mode: string
  gh_accel_custom: string
  gh_accel_prefix: string
}

const NPM_MODES = ['auto', 'official', 'npmmirror', 'custom'] as const
const GH_MODES = ['auto', 'none', 'ghfast', 'ghproxy', 'custom'] as const
/** 自定义 URL 输入后的防抖延迟：停顿这么久才自动保存，避免逐字请求 */
const DEBOUNCE_MS = 500

export function ConfigNetwork() {
  const { t } = useTranslation()
  const [npmMode, setNpmMode] = useState<string>('auto')
  const [npmCustom, setNpmCustom] = useState<string>('')
  const [ghMode, setGhMode] = useState<string>('auto')
  const [ghCustom, setGhCustom] = useState<string>('')

  // 自定义输入的防抖计时器（组件卸载时清理，避免 setState 泄漏）
  const npmTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const ghTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)

  // 组件卸载时清掉未触发的防抖计时器，避免卸载后仍发起保存
  useEffect(() => {
    return () => {
      clearTimeout(npmTimerRef.current)
      clearTimeout(ghTimerRef.current)
    }
  }, [])

  // 载入当前生效配置：进入下载插件界面时同步到本地编辑态
  const { data: config } = useQuery({
    queryKey: ['accel_config'],
    queryFn: async () => {
      const cfg = await invoke<AccelConfig>('get_accel_config')
      setNpmMode(cfg.npm_registry_mode)
      setNpmCustom(cfg.npm_registry_custom)
      setGhMode(cfg.gh_accel_mode)
      setGhCustom(cfg.gh_accel_custom)
      return cfg
    },
  })

  const { mutate: persist, isPending } = useMutation({
    mutationFn: (payload: {
      npmRegistryMode: string
      npmRegistryCustom: string | null
      ghAccelMode: string
      ghAccelCustom: string | null
    }) => invoke<AccelConfig>('update_accel_config', payload),
    onError: (err: unknown) => {
      console.error('[ConfigNetwork] autosave failed:', err)
      toast(t('accel.save_failed'), { variant: 'danger' })
    },
  })

  /** 组装当前完整配置并保存（自动保存共用） */
  function persistCurrent(overrides: {
    npmRegistryMode?: string
    npmRegistryCustom?: string
    ghAccelMode?: string
    ghAccelCustom?: string
  }) {
    const nextNpmMode = overrides.npmRegistryMode ?? npmMode
    const nextGhMode = overrides.ghAccelMode ?? ghMode
    persist({
      npmRegistryMode: nextNpmMode,
      npmRegistryCustom: nextNpmMode === 'custom' ? (overrides.npmRegistryCustom ?? npmCustom) : null,
      ghAccelMode: nextGhMode,
      ghAccelCustom: nextGhMode === 'custom' ? (overrides.ghAccelCustom ?? ghCustom) : null,
    })
  }

  /** npm 源下拉切换：立即保存 */
  function onNpmModeChange(key: unknown) {
    const mode = String(key)
    setNpmMode(mode)
    persistCurrent({ npmRegistryMode: mode })
  }

  /** npm 自定义输入：防抖保存 */
  function onNpmCustomChange(value: string) {
    setNpmCustom(value)
    clearTimeout(npmTimerRef.current)
    npmTimerRef.current = setTimeout(() => {
      persistCurrent({ npmRegistryCustom: value })
    }, DEBOUNCE_MS)
  }

  /** GitHub 镜像下拉切换：立即保存 */
  function onGhModeChange(key: unknown) {
    const mode = String(key)
    setGhMode(mode)
    persistCurrent({ ghAccelMode: mode })
  }

  /** GitHub 自定义输入：防抖保存 */
  function onGhCustomChange(value: string) {
    setGhCustom(value)
    clearTimeout(ghTimerRef.current)
    ghTimerRef.current = setTimeout(() => {
      persistCurrent({ ghAccelCustom: value })
    }, DEBOUNCE_MS)
  }

  return (
    <div className="space-y-3">
      <Surface className="rounded-md p-3">
        <div className="space-y-3">
          {/* npm 源 */}
          <div className="space-y-1.5">
            <Label className="text-xs font-medium text-ink">{t('accel.npm_label')}</Label>
            <Select
              variant="secondary"
              selectedKey={npmMode}
              onSelectionChange={onNpmModeChange}
              className="w-full"
              aria-label={t('accel.npm_label')}
            >
              <Select.Trigger className="rounded-md h-8! py-0 items-center">
                <Select.Value />
                <Select.Indicator />
              </Select.Trigger>
              <Select.Popover className="rounded-md">
                <ListBox>
                  {NPM_MODES.map(mode => (
                    <ListBox.Item key={mode} className="rounded-md min-h-8!" id={mode} textValue={t(`accel.npm_${mode}`)}>
                      {t(`accel.npm_${mode}`)}
                    </ListBox.Item>
                  ))}
                </ListBox>
              </Select.Popover>
            </Select>
            <If cond={npmMode === 'custom'}>
              <Input
                variant="secondary"
                value={npmCustom}
                onChange={e => onNpmCustomChange(e.target.value)}
                placeholder={t('accel.npm_custom_placeholder')}
                className="w-full rounded-md font-mono text-xs"
                aria-label={t('accel.npm_custom')}
              />
            </If>
            <If cond={config != null}>
              <Description className="text-[10px] text-muted/70">
                {t('accel.npm_effective', { url: config!.npm_registry_url })}
              </Description>
            </If>
          </div>

          {/* GitHub 镜像 */}
          <div className="space-y-1.5">
            <Label className="text-xs font-medium text-ink">{t('accel.gh_label')}</Label>
            <Select
              variant="secondary"
              selectedKey={ghMode}
              onSelectionChange={onGhModeChange}
              className="w-full"
              aria-label={t('accel.gh_label')}
            >
              <Select.Trigger className="rounded-md h-8! py-0 items-center">
                <Select.Value />
                <Select.Indicator />
              </Select.Trigger>
              <Select.Popover className="rounded-md">
                <ListBox>
                  {GH_MODES.map(mode => (
                    <ListBox.Item key={mode} className="rounded-md min-h-8!" id={mode} textValue={t(`accel.gh_${mode}`)}>
                      {t(`accel.gh_${mode}`)}
                    </ListBox.Item>
                  ))}
                </ListBox>
              </Select.Popover>
            </Select>
            <If cond={ghMode === 'custom'}>
              <Input
                variant="secondary"
                value={ghCustom}
                onChange={e => onGhCustomChange(e.target.value)}
                placeholder={t('accel.gh_custom_placeholder')}
                className="w-full rounded-md font-mono text-xs"
                aria-label={t('accel.gh_custom')}
              />
            </If>
            <If cond={config != null}>
              <Description className="text-[10px] text-muted/70">
                <If
                  cond={config!.gh_accel_prefix !== ''}
                  then={t('accel.gh_effective', { prefix: config!.gh_accel_prefix })}
                  else={t('accel.gh_effective_direct')}
                />
              </Description>
            </If>
          </div>
        </div>
      </Surface>

      {/* 自动保存指示：保存中显示 spinner，平时保留「下次安装生效」提示 */}
      <div className="flex items-center justify-between gap-2">
        <Description className="text-[10px] text-muted/70">{t('accel.applies_next_install')}</Description>
        <If cond={isPending}>
          <Spinner size="sm" color="current" />
        </If>
      </div>
    </div>
  )
}
