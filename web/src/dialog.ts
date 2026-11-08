import { reactive } from 'vue'

/**
 * 应用内文本输入弹窗。
 *
 * Electron 的渲染进程不支持 `window.prompt()`（会抛 `prompt() is not supported.`），
 * 桌面端里凡是用 prompt 的按钮都会「点不动」。所有需要用户输入一行文本的地方
 * 都改走这里，由 App.vue 里的 <TextPrompt /> 统一渲染。
 */
export interface AskTextOptions {
  title: string
  label?: string
  hint?: string
  value?: string
  placeholder?: string
  confirmText?: string
  /** 允许确认空输入（例如清空能力集）。默认 false。 */
  allowEmpty?: boolean
}

export interface TextPromptState {
  open: boolean
  title: string
  label: string
  hint: string
  value: string
  placeholder: string
  confirmText: string
  allowEmpty: boolean
  resolve: ((value: string | null) => void) | null
}

export const textPrompt = reactive<TextPromptState>({
  open: false,
  title: '',
  label: '',
  hint: '',
  value: '',
  placeholder: '',
  confirmText: '确定',
  allowEmpty: false,
  resolve: null,
})

export function askText(options: AskTextOptions): Promise<string | null> {
  // 上一个弹窗还没结算就再打开时，先把它按「取消」处理，避免旧 Promise 永远悬挂。
  textPrompt.resolve?.(null)
  return new Promise((resolve) => {
    textPrompt.open = true
    textPrompt.title = options.title
    textPrompt.label = options.label ?? ''
    textPrompt.hint = options.hint ?? ''
    textPrompt.value = options.value ?? ''
    textPrompt.placeholder = options.placeholder ?? ''
    textPrompt.confirmText = options.confirmText ?? '确定'
    textPrompt.allowEmpty = options.allowEmpty ?? false
    textPrompt.resolve = resolve
  })
}

export function settleTextPrompt(value: string | null): void {
  if (!textPrompt.open) return
  const resolve = textPrompt.resolve
  textPrompt.open = false
  textPrompt.resolve = null
  resolve?.(value)
}
