'use client'

import { useEffect, useState } from 'react'
import { KeyRound, Loader2, Lock } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { inTauri, invoke } from '@/lib/tauri'
import { useAppStore } from '@/lib/store'
import { markVaultLocked, markVaultSetup, markRuntimeBooting } from '@/lib/runtime'

/** 1Password/Bitwarden/Signal rule: do not use the app until a passphrase
 * exists. Blocks the shell; cannot dismiss. */
export default function VaultGate({ children }: { children: React.ReactNode }) {
  const [gate, setGate] = useState<'loading' | 'open' | 'setup' | 'unlock'>('loading')
  const [pass, setPass] = useState('')
  const [confirm, setConfirm] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const notify = useAppStore((s) => s.notify)

  useEffect(() => {
    if (!inTauri()) {
      setGate('open')
      return
    }
    void invoke<{ needsSetup?: boolean; ok?: boolean; mode?: string }>('vault_key_status')
      .then((s) => {
        if (s.mode === 'unlock') {
          markVaultLocked()
          setGate('unlock')
        } else if (s.needsSetup || s.mode === 'setup' || s.mode === 'wrap') {
          markVaultSetup()
          setGate('setup')
        } else {
          markRuntimeBooting()
          setGate('open')
        }
      })
      .catch((cause) => {
        markVaultSetup('Vault status could not be verified. Create or repair the vault before continuing.')
        setError(cause instanceof Error ? cause.message : 'Vault status could not be verified')
        setGate('setup')
      })
  }, [])

  const submit = async () => {
    if (pass.length < 8) {
      setError('Passphrase must be at least 8 characters')
      return
    }
    if (gate === 'setup' && pass !== confirm) {
      setError('Passphrases do not match')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const cmd = gate === 'setup' ? 'vault_setup' : 'vault_unlock'
      await invoke(cmd, { passphrase: pass })
      notify(gate === 'setup' ? 'Vault created' : 'Vault unlocked')
      markRuntimeBooting()
      setGate('open')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Vault rejected the passphrase')
    } finally {
      setBusy(false)
    }
  }

  if (gate === 'loading') {
    return (
      <div className="flex h-screen items-center justify-center bg-background">
        <Loader2 className="h-5 w-5 animate-spin text-brand" />
      </div>
    )
  }
  if (gate === 'open') return <>{children}</>

  const setup = gate === 'setup'
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-background">
      <div className="w-full max-w-md rounded-xl border border-border bg-card p-6 shadow-xl">
        <div className="mb-4 flex items-center gap-2">
          {setup ? (
            <KeyRound className="h-5 w-5 text-brand" />
          ) : (
            <Lock className="h-5 w-5 text-brand" />
          )}
          <h1 className="text-sm font-medium text-foreground">
            {setup ? 'Create your vault passphrase' : 'Unlock AgentCowork'}
          </h1>
        </div>
        <p className="mb-4 text-xs text-muted-foreground">
          {setup
            ? '      Keys, chats and secrets are encrypted with Argon2id. There is no silent generated key — this passphrase is required before the app runs.'
            : 'Enter the passphrase for this device’s vault.'}
        </p>
        <label className="mb-2 block text-[10px] uppercase tracking-wider text-muted-foreground">
          Passphrase
        </label>
        <input
          type="password"
          value={pass}
          onChange={(e) => setPass(e.target.value)}
          className="mb-3 h-9 w-full rounded-md border border-border bg-background px-3 font-mono text-xs"
          autoFocus
        />
        {setup && (
          <>
            <label className="mb-2 block text-[10px] uppercase tracking-wider text-muted-foreground">
              Confirm
            </label>
            <input
              type="password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              className="mb-3 h-9 w-full rounded-md border border-border bg-background px-3 font-mono text-xs"
            />
          </>
        )}
        {error && <p className="mb-3 text-[11px] text-red-400">{error}</p>}
        <Button
          className="h-8 w-full bg-brand text-xs text-black hover:bg-brand"
          disabled={busy}
          onClick={() => void submit()}
        >
          {busy ? 'Working…' : setup ? 'Create vault' : 'Unlock'}
        </Button>
      </div>
    </div>
  )
}
