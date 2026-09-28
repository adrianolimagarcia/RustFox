import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { ApiError, api } from '../api/client'

/**
 * Public magic-link claim form (secure-store Slice 2).
 * Opaque one-shot token is the credential — no portal login required.
 * Input is type=password (masked); value never appears in Telegram.
 */
export const Route = createFileRoute('/secrets/claim/$token')({
  component: SecretClaimPage,
})

type ClaimMeta = {
  id: string
  name: string
  expiresAt: number
  status: string
}

function SecretClaimPage() {
  const { token } = Route.useParams()
  const [meta, setMeta] = useState<ClaimMeta | null>(null)
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState(false)
  const [cancelled, setCancelled] = useState(false)

  useEffect(() => {
    let cancelledFetch = false
    setError(null)
    api
      .getSecretClaim(token)
      .then((m) => {
        if (!cancelledFetch) setMeta(m)
      })
      .catch((err: unknown) => {
        if (cancelledFetch) return
        const status = (err as ApiError).status
        setError(
          status === 404
            ? 'This claim link is invalid or has expired.'
            : 'Could not load pending secret.',
        )
      })
    return () => {
      cancelledFetch = true
    }
  }, [token])

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    if (!value || busy || done) return
    setBusy(true)
    setError(null)
    try {
      await api.submitSecretClaim(token, value)
      setDone(true)
      setValue('')
    } catch (err) {
      const status = (err as ApiError).status
      setError(
        status === 404
          ? 'This claim link is invalid or has expired.'
          : (err as ApiError).message || 'Failed to store secret.',
      )
    } finally {
      setBusy(false)
    }
  }

  async function cancel() {
    if (busy || done || cancelled) return
    setBusy(true)
    setError(null)
    try {
      await api.cancelSecretClaim(token)
      setCancelled(true)
      setValue('')
    } catch (err) {
      setError((err as ApiError).message || 'Cancel failed.')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="login-wrap">
      <form className="login-card" onSubmit={submit}>
        <h1>🔐 Enter secret</h1>
        {meta ? (
          <p>
            RustFox needs <code className="mono">{meta.name}</code>. Value is masked and
            stored on the host — never pasted into Telegram chat.
          </p>
        ) : (
          <p>Loading pending secret…</p>
        )}

        {done ? (
          <div className="banner" role="status" style={{ marginTop: 12 }}>
            Secret stored. You can close this page.
          </div>
        ) : null}

        {cancelled ? (
          <div className="banner warn" role="status" style={{ marginTop: 12 }}>
            Pending request cancelled. No value was stored.
          </div>
        ) : null}

        {!done && !cancelled && meta ? (
          <>
            <div className="field">
              <label htmlFor="secret-value">{meta.name}</label>
              <input
                id="secret-value"
                type="password"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder="••••••••"
                autoComplete="off"
                autoFocus
                disabled={busy}
              />
            </div>

            {error ? <div className="login-error">{error}</div> : null}

            <button
              type="submit"
              className="primary"
              style={{ width: '100%' }}
              disabled={busy || !value}
            >
              {busy ? 'Saving…' : 'Save secret'}
            </button>
            <button
              type="button"
              className="ghost"
              style={{ width: '100%', marginTop: 8 }}
              onClick={() => void cancel()}
              disabled={busy}
            >
              Cancel request
            </button>
          </>
        ) : null}

        {!meta && error ? <div className="login-error">{error}</div> : null}
      </form>
    </div>
  )
}
