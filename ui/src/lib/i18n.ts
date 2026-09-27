// P11.3 — i18n + RTL infrastructure. Every user-facing string lives in a
// locale dict (English is the default + fallback); `useLocale` exposes the
// active locale + `t()` + a switcher, and binds `document.dir` for RTL
// (Arabic/Hebrew). This is the plumbing — components migrate string-by-string
// to `t()`; un-migrated hardcoded strings still render (English), so nothing
// regresses while the migration is in flight.

import { useCallback, useEffect, useMemo, useState } from 'react'
import { getLocalItem } from './storage-compat'

export type Locale = 'en' | 'ar' | 'he'
export type LocaleDict = Record<string, string>

const PREFIX = 'agentcowork.settings.'

const en: LocaleDict = {
  'app.name': 'AgentCowork',
  'common.open': 'Open',
  'common.save': 'Save',
  'common.cancel': 'Cancel',
  'common.close': 'Close',
  'common.delete': 'Delete',
  'common.confirm': 'Confirm',
  'common.search': 'Search',
  'common.loading': 'Loading…',
  'common.empty': 'Nothing here yet',
  'common.error': 'Something went wrong',
  'common.retry': 'Retry',
  'common.back': 'Back',
  'common.next': 'Next',
  'common.skip': 'Skip',
  'common.done': 'Done',
  'common.start': 'Start',
  'common.browse': 'Browse',
  'common.live': 'Live',
  'common.paused': 'Paused',
  'chat.placeholder': 'Message AgentCowork…',
  'chat.send': 'Send',
  'chat.thinking': 'Thinking…',
  'chat.newSession': 'New chat',
  'onboarding.welcome': 'Welcome to AgentCowork',
  'onboarding.subtitle': 'Your local AI copilot — keys stay on this device.',
  'onboarding.addKey': 'Add your first API key',
  'onboarding.addKeyDesc': 'Bring your own key — OpenAI, Anthropic, DeepSeek, NVIDIA and more.',
  'onboarding.startChat': 'Start your first chat',
  'onboarding.success': "You're all set",
  'onboarding.successDesc': 'Ask anything — AgentCowork plans, browses, edits files and automates.',
  'empty.messages': 'No messages yet — ask something to get started.',
  'empty.keys': 'No API keys yet — add one to start chatting.',
  'empty.files': 'No files open — browse the folder view to pick one.',
  'empty.memory': 'No memories yet — AgentCowork learns as you work.',
  'empty.automations': 'No automations yet — create one or pick a template.',
  'error.network': 'Network unreachable — check your connection and retry.',
  'error.keyRevoked': 'This API key was revoked — add another key or fix the credential.',
  'error.provider5xx': 'The provider is having issues (5xx) — try again in a moment.',
  'error.budget': 'Chat budget exceeded — raise the cap in Settings to continue.',
  'error.unknown': 'Something went wrong — details in the activity log.',
  'loading.ttft': 'Waiting for the first token…',
  'loading.compaction': 'Compacting context…',
  'loading.tool': 'Running tool…',
  'loading.agent': 'Agent is working…',
  'guard.approve': 'Approve',
  'guard.reject': 'Reject',
  'guard.dismiss': 'Dismiss',
  'guard.confirm': 'Confirm',
  'guard.deny': 'Deny',
  'agent.pause': 'Pause',
  'agent.resume': 'Resume',
  'agent.describeChanges': 'Describe what you changed (the agent will continue from here)',
  'takeover.live': '● Live',
  'takeover.paused': '⏸ Paused',
  'automations.new': 'New automation',
  'automations.fromTemplate': 'From template',
  'automations.describe': 'Describe your automation in plain words…',
  'settings.appearance': 'Appearance',
  'settings.language': 'Language',
  'settings.highContrast': 'High contrast',
  'settings.fontScale': 'Text size',
  'settings.keyboard': 'Keyboard shortcuts',
  'folder.view': 'Folder',
  'shell.view': 'Shell',
  'browse.view': 'Browse',
  'code.view': 'Code',
}

const ar: LocaleDict = {
  'app.name': 'إيفري أيوس',
  'common.open': 'فتح',
  'common.save': 'حفظ',
  'common.cancel': 'إلغاء',
  'common.search': 'بحث',
  'common.loading': 'جارٍ التحميل…',
  'common.empty': 'لا يوجد شيء هنا بعد',
  'common.error': 'حدث خطأ ما',
  'common.retry': 'إعادة المحاولة',
  'chat.placeholder': 'راسل إيفري أيوس…',
  'chat.send': 'إرسال',
  'onboarding.welcome': 'مرحباً بك في إيفري أيوس',
  'settings.language': 'اللغة',
}

const he: LocaleDict = {
  'app.name': 'אוורייאוס',
  'common.open': 'פתיחה',
  'common.save': 'שמירה',
  'common.cancel': 'ביטול',
  'common.search': 'חיפוש',
  'common.loading': 'טוען…',
  'common.empty': 'אין כאן כלום עדיין',
  'common.error': 'אירעה שגיאה',
  'common.retry': 'נסה שוב',
  'chat.placeholder': 'כתוב לאוורייאוס…',
  'chat.send': 'שלח',
  'onboarding.welcome': 'ברוכים הבאים לאוורייאוס',
  'settings.language': 'שפה',
}

const DICTS: Record<Locale, LocaleDict> = { en, ar, he }

/** RTL locales — sets `dir="rtl"` on <html>. */
const RTL: Locale[] = ['ar', 'he']

function readLocale(): Locale {
  if (typeof window === 'undefined') return 'en'
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const raw = getLocalItem(PREFIX + 'locale')
    if (raw === 'ar' || raw === 'he' || raw === 'en') return raw
  } catch {
    /* ignore */
  }
  return 'en'
}

export function useLocale() {
  const [locale, setLocaleState] = useState<Locale>(readLocale)

  useEffect(() => {
    document.documentElement.lang = locale
    document.documentElement.dir = RTL.includes(locale) ? 'rtl' : 'ltr'
  }, [locale])

  const t = useCallback(
    (key: string): string => DICTS[locale][key] ?? en[key] ?? key,
    [locale]
  )

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next)
    try {
      window.localStorage.setItem(PREFIX + 'locale', next)
    } catch {
      /* ignore */
    }
  }, [])

  const dir = useMemo(() => (RTL.includes(locale) ? 'rtl' : 'ltr'), [locale])
  return { locale, setLocale, t, dir }
}
