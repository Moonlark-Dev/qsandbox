<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { settleTextPrompt, textPrompt } from '../dialog'

const draft = ref('')
const input = ref<HTMLInputElement | null>(null)

watch(
  () => textPrompt.open,
  async (open) => {
    if (!open) return
    draft.value = textPrompt.value
    await nextTick()
    input.value?.focus()
    input.value?.select()
  },
)

function confirm() {
  if (!textPrompt.allowEmpty && !draft.value.trim()) return
  settleTextPrompt(draft.value)
}
</script>

<template>
  <div v-if="textPrompt.open" class="modal-mask" style="z-index: 70" @click.self="settleTextPrompt(null)">
    <div class="modal" style="width: min(440px, calc(100vw - 32px))">
      <h3>{{ textPrompt.title }}</h3>
      <label class="field">
        <span v-if="textPrompt.label">{{ textPrompt.label }}</span>
        <input
          ref="input"
          v-model="draft"
          class="input"
          :placeholder="textPrompt.placeholder"
          @keyup.enter="confirm"
          @keyup.esc="settleTextPrompt(null)"
        />
      </label>
      <p v-if="textPrompt.hint" class="small faint" style="margin: 8px 0 0">{{ textPrompt.hint }}</p>
      <div class="modal-actions">
        <button class="btn ghost" @click="settleTextPrompt(null)">取消</button>
        <button class="btn primary" :disabled="!textPrompt.allowEmpty && !draft.trim()" @click="confirm">
          {{ textPrompt.confirmText }}
        </button>
      </div>
    </div>
  </div>
</template>
