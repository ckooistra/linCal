<script lang="ts">
  import { onMount, tick } from "svelte";
  import { modal } from "../services/modal";

  interface Props {
    /** Resolves to an error message, or null on success. */
    onsubmit: (serverUrl: string, username: string, password: string) => Promise<string | null>;
    onclose: () => void;
    /** Stops a connection attempt in progress (the dialog then closes). */
    oncancel?: () => void;
  }

  let { onsubmit, onclose, oncancel }: Props = $props();

  /** Closing while connecting cancels the attempt. */
  function close() {
    if (busy) oncancel?.();
    onclose();
  }

  let serverUrl = $state("");
  let username = $state("");
  let password = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let cancelButton: HTMLButtonElement | undefined = $state();
  let first: HTMLInputElement;

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    if (!serverUrl.trim() || !username.trim() || !password) {
      error = "Fill in the server address, user name and password.";
      return;
    }
    busy = true;
    error = null;
    // The fields are disabled while busy: keep focus (and Escape) in the dialog.
    void tick().then(() => cancelButton?.focus());
    try {
      error = await onsubmit(serverUrl.trim(), username.trim(), password);
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
    aria-labelledby="add-account-title"
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
      <h2 id="add-account-title">Add CalDAV account</h2>
      <p class="hint">
        Works with Nextcloud, Fastmail, Radicale, Baïkal and other CalDAV servers. Use an app password where the
        server offers one (Nextcloud: Settings → Security → Devices &amp; sessions).
      </p>
      <fieldset disabled={busy}>
        <label>
          <span>Server address</span>
          <input
            bind:this={first}
            bind:value={serverUrl}
            type="url"
            placeholder="https://cloud.example.org"
            autocomplete="url"
            spellcheck="false"
          />
        </label>
        <label>
          <span>User name</span>
          <input bind:value={username} autocomplete="username" spellcheck="false" />
        </label>
        <label>
          <span>Password or app password</span>
          <input bind:value={password} type="password" autocomplete="current-password" />
        </label>
      </fieldset>
      <p class="hint">The password is stored in the system keyring (KWallet or GNOME Keyring), never in the calendar database.</p>
      {#if error}
        <p class="error" role="alert">{error}</p>
      {/if}
      <div class="actions">
        {#if busy}<span class="status" role="status">Connecting…</span>{/if}
        <span class="spacer"></span>
        <button type="button" bind:this={cancelButton} onclick={close}>Cancel</button>
        <button type="submit" class="primary" disabled={busy}>Connect</button>
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
