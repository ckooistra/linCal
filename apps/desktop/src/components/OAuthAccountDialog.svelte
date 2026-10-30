<script lang="ts">
  import { onMount, tick } from "svelte";
  import { modal } from "../services/modal";

  interface Props {
    provider: "google" | "microsoft";
    /** Client ID built into this app by its distributor, if any. */
    builtInClientId: string | null;
    /** Resolves to an error message, or null on success. */
    onsubmit: (clientId: string, clientSecret: string | null) => Promise<string | null>;
    onclose: () => void;
    /** Stops a sign-in in progress (the dialog then closes). */
    oncancel?: () => void;
  }

  let { provider, builtInClientId, onsubmit, onclose, oncancel }: Props = $props();

  const TEXT = {
    google: {
      name: "Google",
      placeholder: "1234-abc.apps.googleusercontent.com",
      registration:
        'Create a "Desktop app" OAuth client in the Google Cloud console with the Google Calendar API enabled.',
    },
    microsoft: {
      name: "Microsoft",
      placeholder: "00000000-0000-0000-0000-000000000000",
      registration:
        'Register an app in Microsoft Entra ID ("Personal and work accounts") with the redirect URI http://localhost under "Mobile and desktop applications" and the Calendars.ReadWrite permission.',
    },
  } as const;
  const text = $derived(TEXT[provider]);

  // svelte-ignore state_referenced_locally
  let clientId = $state(builtInClientId ?? "");
  let clientSecret = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let cancelButton: HTMLButtonElement | undefined = $state();
  let first: HTMLElement;

  function close() {
    if (busy) oncancel?.();
    onclose();
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    if (!clientId.trim()) {
      error = `Enter the OAuth client ID of your ${text.name} app registration.`;
      return;
    }
    busy = true;
    error = null;
    // The fields are disabled while busy: keep focus (and Escape) in the dialog.
    void tick().then(() => cancelButton?.focus());
    try {
      error = await onsubmit(clientId.trim(), clientSecret.trim() || null);
    } finally {
      busy = false;
    }
  }

  onMount(() => first.focus());
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && !busy && close()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="oauth-title"
    tabindex="-1"
    use:modal
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    }}
  >
    <form onsubmit={submit} novalidate>
      <h2 id="oauth-title">Add {text.name} account</h2>
      <p class="hint">
        Signing in happens in your web browser; Universal Calendar never sees your {text.name} password. Access tokens are
        kept in the system keyring.
      </p>
      <fieldset disabled={busy}>
        {#if builtInClientId}
          <p class="hint">This build includes its own {text.name} app registration.</p>
        {/if}
        <label>
          <span>OAuth client ID</span>
          <input
            bind:this={first}
            bind:value={clientId}
            placeholder={text.placeholder}
            spellcheck="false"
            autocomplete="off"
          />
        </label>
        {#if provider === "google"}
          <label>
            <span>Client secret (only if your client type requires one)</span>
            <input bind:value={clientSecret} type="password" autocomplete="off" />
          </label>
        {/if}
      </fieldset>
      <p class="hint">{text.registration} See docs/provider-integration.md.</p>
      {#if error}
        <p class="error" role="alert">{error}</p>
      {/if}
      <div class="actions">
        {#if busy}<span class="status" role="status">Waiting for you to finish signing in in the browser…</span>{/if}
        <span class="spacer"></span>
        <button type="button" bind:this={cancelButton} onclick={close}>Cancel</button>
        <button type="submit" class="primary" disabled={busy}>Sign in with {text.name}</button>
      </div>
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--uc-backdrop);
    display: grid;
    place-items: center;
    z-index: 50;
  }
  .dialog {
    width: min(30rem, calc(100vw - 2rem));
    background: var(--uc-surface);
    color: var(--uc-text);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
    padding: 1.25rem;
    outline: none;
  }
  h2 {
    margin: 0 0 0.5rem;
    font-size: 1.1rem;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0.75rem 0;
    display: grid;
    gap: 0.6rem;
  }
  label {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
  }
  label > span,
  .hint,
  .status {
    color: var(--uc-text-muted);
    font-size: 0.8rem;
  }
  .hint {
    margin: 0;
  }
  .error {
    color: var(--uc-danger);
    margin: 0.75rem 0 0;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 1rem;
  }
  .spacer {
    flex: 1;
  }
</style>
