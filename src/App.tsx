import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState, type FormEvent, type MouseEvent } from "react";
import "./App.css";

type Theme = "glass" | "cutout" | "signal";
type DisplayMode = "desktop" | "pinned";
type ConnectionState = "waiting" | "connected" | "disconnected" | "error" | "disabled" | "untrusted";

type Geometry = {
  x: number;
  y: number;
  width: number;
  height: number;
  scale_factor: number;
  monitor_name: string | null;
};

type WindowState = {
  schema_version: number;
  mode: DisplayMode;
  locked: boolean;
  visible: boolean;
  geometry: Geometry | null;
};

type Settings = {
  schema_version: number;
  theme: Theme;
  refresh_interval_seconds: number;
  provider_order: string[];
  disabled_providers: string[];
  onboarding_complete: boolean;
};

type UsageWindow = {
  id: string;
  label: string;
  used_percent: number;
  resets_at: number | null;
  window_minutes: number | null;
  display: "ring" | "bar";
};

type Balance = {
  id: string;
  label: string;
  amount: string | null;
  unit: string | null;
  known: boolean;
};

type Provider = {
  schema_version: number;
  id: string;
  name: string;
  accent: string | null;
  state: ConnectionState;
  status_message: string;
  observed_at: number | null;
  last_attempt_at: number | null;
  error_code: string | null;
  windows: UsageWindow[];
  balances: Balance[];
  refreshing: boolean;
};

type Adapter = {
  id: string;
  name: string;
  command: string;
  args: string[];
  refresh_interval_seconds: number;
  trusted: boolean;
  trust_changed: boolean;
  valid: boolean;
  diagnostic: string | null;
};

type Aggregate = {
  schema_version: number;
  surface: string;
  app_version: string;
  settings: Settings;
  window: WindowState;
  providers: Provider[];
  adapters: Adapter[];
  claude_capture: {
    state: "not_installed" | "installed" | "conflict" | "settings_invalid";
    message: string;
    settings_path: string;
  };
  autostart_enabled: boolean;
  paths: { config: string; cache: string; state: string; adapters: string };
};

type ActionResult = { ok: boolean; code: string; message: string };

const resizeEdges = [
  "north",
  "north-east",
  "east",
  "south-east",
  "south",
  "south-west",
  "west",
  "north-west",
] as const;

function App() {
  const [app, setApp] = useState<Aggregate | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  const reload = useCallback(async () => {
    try {
      setApp(await invoke<Aggregate>("get_app_state"));
      setError(null);
    } catch (reason) {
      setError(String(reason));
    }
  }, []);

  useEffect(() => {
    void reload();
    const unlisteners = [
      listen<Provider[]>("providers-changed", (event) =>
        setApp((current) => (current ? { ...current, providers: orderProviders(event.payload, current.settings) } : current)),
      ),
      listen<Settings>("settings-changed", (event) =>
        setApp((current) =>
          current
            ? {
                ...current,
                settings: event.payload,
                providers: orderProviders(current.providers, event.payload),
              }
            : current,
        ),
      ),
      listen<WindowState>("widget-state", (event) =>
        setApp((current) => (current ? { ...current, window: event.payload } : current)),
      ),
    ];
    const tick = window.setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => {
      window.clearInterval(tick);
      for (const unlisten of unlisteners) void unlisten.then((dispose) => dispose());
    };
  }, [reload]);

  useEffect(() => {
    document.documentElement.dataset.theme = app?.settings.theme ?? "signal";
    document.body.dataset.surface = app?.surface ?? "loading";
  }, [app?.settings.theme, app?.surface]);

  if (!app) {
    return <main className="loading">{error ?? "Starting Agent Gauge…"}</main>;
  }

  return app.surface === "settings" ? (
    <SettingsView app={app} reload={reload} setError={setError} />
  ) : (
    <Widget app={app} now={now} error={error} setError={setError} />
  );
}

function Widget({
  app,
  now,
  error,
  setError,
}: {
  app: Aggregate;
  now: number;
  error: string | null;
  setError: (message: string | null) => void;
}) {
  const providers = app.providers.filter(
    (provider) =>
      provider.state !== "disabled" &&
      provider.state !== "untrusted" &&
      !app.settings.disabled_providers.includes(provider.id),
  );

  function startDrag(event: MouseEvent<HTMLElement>) {
    if (event.button !== 0 || app.window.locked) return;
    event.preventDefault();
    void invoke("begin_drag").catch((reason) => setError(String(reason)));
  }

  function startResize(edge: (typeof resizeEdges)[number], event: MouseEvent<HTMLDivElement>) {
    if (event.button !== 0 || app.window.locked) return;
    event.preventDefault();
    event.stopPropagation();
    void invoke("begin_resize", { edge }).catch((reason) => setError(String(reason)));
  }

  return (
    <main className={`widget ${app.window.locked ? "" : "widget--unlocked"}`} onMouseDown={startDrag}>
      <section className="widget__surface">
        <header className="widget__topline">
          <span className="wordmark">AGENT GAUGE</span>
          <span className="widget__mode">{app.window.mode === "pinned" ? "PINNED" : "DESKTOP"}</span>
        </header>

        <div className="provider-grid">
          {providers.length ? (
            providers.map((provider) => <ProviderCard key={provider.id} provider={provider} now={now} />)
          ) : (
            <div className="empty-state">
              <strong>No trackers enabled</strong>
              <span>Open Settings from the tray to choose providers.</span>
            </div>
          )}
        </div>

        {!app.window.locked ? (
          <div className="editing-note">Layout unlocked · drag anywhere · resize at the edges</div>
        ) : null}
        {error ? <div className="widget-error">{error}</div> : null}
      </section>

      {!app.window.locked
        ? resizeEdges.map((edge) => (
            <div
              aria-hidden="true"
              className={`resize-handle resize-handle--${edge}`}
              key={edge}
              onMouseDown={(event) => startResize(edge, event)}
            />
          ))
        : null}
    </main>
  );
}

function ProviderCard({ provider, now }: { provider: Provider; now: number }) {
  const freshness = freshnessFor(provider, now);
  const ring = provider.windows.find((window) => window.display === "ring");
  const bars = provider.windows.filter((window) => window.display === "bar");
  return (
    <article
      className={`provider-card provider-card--${freshness.tone} ${provider.refreshing ? "provider-card--refreshing" : ""}`}
      style={{ "--provider-accent": provider.accent ?? "var(--accent)" } as React.CSSProperties}
    >
      <header className="provider-card__header">
        <div className="provider-name">
          <span className="provider-dot" />
          <strong>{provider.name}</strong>
        </div>
        <span className="freshness"><span className="freshness__glyph">{freshness.glyph}</span>{freshness.label}</span>
      </header>

      {ring ? <RingMetric window={ring} now={now} /> : provider.windows.length ? null : <ConnectionMessage provider={provider} />}
      {bars.map((window) => <BarMetric key={window.id} window={window} now={now} />)}

      {provider.balances.filter((balance) => balance.known).map((balance) => (
        <div className="balance-row" key={balance.id}>
          <span>{balance.label}</span>
          <strong>{formatBalance(balance)}</strong>
        </div>
      ))}

      {provider.error_code ? <div className="provider-warning">{provider.status_message}</div> : null}
    </article>
  );
}

function RingMetric({ window, now }: { window: UsageWindow; now: number }) {
  const used = clamp(window.used_percent);
  const circumference = 2 * Math.PI * 34;
  return (
    <div className="ring-metric">
      <div className="usage-ring" aria-label={`${formatPercent(window.used_percent)} used`}>
        <svg viewBox="0 0 80 80" role="img">
          <circle className="usage-ring__track" cx="40" cy="40" r="34" />
          <circle
            className="usage-ring__fill"
            cx="40"
            cy="40"
            r="34"
            strokeDasharray={circumference}
            strokeDashoffset={circumference * (1 - used / 100)}
          />
        </svg>
        <div className="usage-ring__value"><strong>{formatPercent(window.used_percent)}</strong><span>used</span></div>
      </div>
      <div className="metric-copy">
        <span className="metric-label">{window.label}</span>
        <strong>{formatReset(window.resets_at, now)}</strong>
        <span>{formatAbsolute(window.resets_at)}</span>
      </div>
    </div>
  );
}

function BarMetric({ window, now }: { window: UsageWindow; now: number }) {
  return (
    <div className="bar-metric">
      <div className="bar-metric__labels"><span>{window.label}</span><strong>{formatPercent(window.used_percent)} used</strong></div>
      <div className="usage-bar"><span style={{ width: `${clamp(window.used_percent)}%` }} /></div>
      <div className="bar-metric__reset">{formatReset(window.resets_at, now)} · {formatAbsolute(window.resets_at)}</div>
    </div>
  );
}

function ConnectionMessage({ provider }: { provider: Provider }) {
  return (
    <div className="connection-message">
      <span>{provider.state === "waiting" ? "○" : provider.state === "untrusted" ? "◇" : "!"}</span>
      <div><strong>{provider.status_message}</strong><small>{provider.id === "claude" ? "Use Claude Code normally after connecting capture." : "Open Settings for diagnostics."}</small></div>
    </div>
  );
}

function SettingsView({
  app,
  reload,
  setError,
}: {
  app: Aggregate;
  reload: () => Promise<void>;
  setError: (message: string | null) => void;
}) {
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [addingTracker, setAddingTracker] = useState(false);
  const [trackerName, setTrackerName] = useState("");

  const run = useCallback(
    async (command: string, args?: Record<string, unknown>) => {
      setBusy(true);
      try {
        const result = await invoke<ActionResult>(command, args);
        setNotice(result.message);
        if (!result.ok) setError(result.message);
        await reload();
      } catch (reason) {
        setError(String(reason));
      } finally {
        setBusy(false);
      }
    },
    [reload, setError],
  );

  const update = useCallback(
    (patch: Record<string, unknown>) => run("apply_settings", { patch }),
    [run],
  );

  const adapterIds = useMemo(() => new Set(app.adapters.map((adapter) => adapter.id)), [app.adapters]);
  const visibleProviders = useMemo(
    () => orderProviders(app.providers, app.settings).filter((provider) => !adapterIds.has(provider.id)),
    [app.providers, app.settings, adapterIds],
  );
  const exampleAdapter = app.adapters.find((adapter) => adapter.id === "sample");
  const customAdapters = app.adapters.filter((adapter) => adapter.id !== "sample");

  function toggleProvider(id: string, enabled: boolean) {
    const disabled = new Set(app.settings.disabled_providers);
    if (enabled) disabled.delete(id);
    else disabled.add(id);
    void update({ disabled_providers: [...disabled] });
  }

  function moveProvider(id: string, delta: -1 | 1) {
    const order = [...app.settings.provider_order];
    const current = order.indexOf(id);
    if (current < 0 || current + delta < 0 || current + delta >= order.length) return;
    [order[current], order[current + delta]] = [order[current + delta], order[current]];
    void update({ provider_order: order });
  }

  function addTracker(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!trackerName.trim()) return;
    void (async () => {
      await run("create_adapter_scaffold", { name: trackerName.trim() });
      setTrackerName("");
      setAddingTracker(false);
    })();
  }

  return (
    <main className="settings-shell">
      <header className="settings-header">
        <div><span className="settings-kicker">AGENT GAUGE</span><h1>Settings</h1></div>
        <button className="icon-button" onClick={() => void run("close_settings")} aria-label="Close settings">
          <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M3 3l10 10M13 3L3 13" /></svg>
        </button>
      </header>

      {!app.settings.onboarding_complete ? (
        <section className="welcome-card">
          <span className="welcome-card__mark">◒</span>
          <div><h2>Your usage, without the window clutter.</h2><p>Agent Gauge reads signed-in local CLIs and never makes a model call. Claude capture is connected automatically with safe restore support.</p></div>
          <button onClick={() => void update({ onboarding_complete: true })}>Continue</button>
        </section>
      ) : null}

      <div className="settings-layout">
        <section className="settings-section">
          <div className="section-heading"><div><span>01</span><h2>Appearance</h2></div><p>Choose how the widget sits on your desktop.</p></div>
          <div className="theme-grid">
            {(["glass", "cutout", "signal"] as Theme[]).map((theme) => (
              <button
                className={`theme-option theme-option--${theme} ${app.settings.theme === theme ? "is-selected" : ""}`}
                key={theme}
                onClick={() => void update({ theme })}
              >
                <span className="theme-preview"><i /><i /><i /></span>
                <strong>{title(theme)}</strong>
                <small>{theme === "glass" ? "Translucent" : theme === "cutout" ? "Wallpaper-first" : "Opaque & reliable"}</small>
              </button>
            ))}
          </div>
        </section>

        <section className="settings-section">
          <div className="section-heading"><div><span>02</span><h2>Widget</h2></div><p>Layering and layout behavior.</p></div>
          <div className="setting-row">
            <div><strong>Display layer</strong><small>Desktop stays behind apps; pinned stays above them.</small></div>
            <div className="segmented">
              <button className={app.window.mode === "desktop" ? "is-active" : ""} onClick={() => void run("set_display_mode", { mode: "desktop" })}>Desktop</button>
              <button className={app.window.mode === "pinned" ? "is-active" : ""} onClick={() => void run("set_display_mode", { mode: "pinned" })}>Pinned</button>
            </div>
          </div>
          <div className="setting-row">
            <div><strong>Layout</strong><small>Unlock temporarily to drag and resize.</small></div>
            <button className="secondary-button" onClick={() => void run("toggle_layout_lock")}>{app.window.locked ? "Unlock layout" : "Lock layout"}</button>
          </div>
          <div className="setting-row">
            <div><strong>Widget visibility</strong><small>The tray remains available when hidden.</small></div>
            <Switch checked={app.window.visible} onChange={(value) => void run("set_widget_visible", { visible: value })} label="Widget visible" />
          </div>
        </section>

        <section className="settings-section">
          <div className="section-heading"><div><span>03</span><h2>Trackers</h2></div><p>Read-only local provider connections.</p></div>
          <div className="tracker-list">
            {visibleProviders.map((provider, index) => (
              <div className="tracker-row" key={provider.id}>
                <span className="tracker-accent" style={{ background: provider.accent ?? "var(--accent)" }} />
                <div className="tracker-copy"><strong>{provider.name}</strong><small>{provider.status_message}</small></div>
                <span className={`tracker-state tracker-state--${provider.state}`}>{provider.refreshing ? "Refreshing" : provider.state}</span>
                <div className="reorder-buttons">
                  <button disabled={index === 0} onClick={() => moveProvider(provider.id, -1)} aria-label={`Move ${provider.name} up`}>↑</button>
                  <button disabled={index === visibleProviders.length - 1} onClick={() => moveProvider(provider.id, 1)} aria-label={`Move ${provider.name} down`}>↓</button>
                </div>
                <button className="text-button" onClick={() => void run("refresh_provider", { providerId: provider.id })}>Refresh</button>
                <Switch checked={!app.settings.disabled_providers.includes(provider.id)} onChange={(value) => toggleProvider(provider.id, value)} label={`${provider.name} enabled`} />
              </div>
            ))}
          </div>

          <div className="integration-card">
            <div><strong>Claude Code capture</strong><small>{app.claude_capture.message}</small><code>{app.claude_capture.settings_path}</code></div>
            {app.claude_capture.state === "installed" ? (
              <button className="secondary-button" onClick={() => void run("remove_claude_capture")}>Disconnect</button>
            ) : (
              <button onClick={() => void run("install_claude_capture")}>Connect Claude</button>
            )}
          </div>

          <div className="additional-heading">
            <div>
              <strong>Additional trackers</strong>
              <small>Add up to five local provider adapters. New starters are disabled and cannot execute until you explicitly trust their files.</small>
            </div>
            <span>{customAdapters.length} / 5</span>
            <button disabled={customAdapters.length >= 5} onClick={() => setAddingTracker((value) => !value)}>
              {addingTracker ? "Cancel" : "Add tracker"}
            </button>
          </div>

          {addingTracker ? (
            <form className="adapter-form" onSubmit={addTracker}>
              <label htmlFor="tracker-name">Tracker name</label>
              <div><input id="tracker-name" autoFocus maxLength={60} value={trackerName} onChange={(event) => setTrackerName(event.target.value)} placeholder="e.g. Gemini" /><button disabled={!trackerName.trim() || busy} type="submit">Create disabled starter</button></div>
              <small>This creates a local adapter folder for you or an AI coding agent to connect. It does not contact a provider or run automatically.</small>
            </form>
          ) : null}

          {customAdapters.map((adapter) => (
            <div className="adapter-card" key={adapter.id}>
              <div><strong>{adapter.name}</strong><small>{adapter.diagnostic ?? `Runs every ${Math.round(adapter.refresh_interval_seconds / 60)} min`}</small><code>{adapter.command} {adapter.args.join(" ")}</code></div>
              <div className="adapter-actions">
                {adapter.trusted ? <button className="text-button" onClick={() => void run("test_adapter", { adapterId: adapter.id })}>Test</button> : null}
                {adapter.trusted ? (
                  <button className="secondary-button" onClick={() => void run("revoke_adapter", { adapterId: adapter.id })}>Revoke</button>
                ) : (
                  <button disabled={!adapter.valid} onClick={() => void run("trust_adapter", { adapterId: adapter.id })}>Trust & enable</button>
                )}
              </div>
            </div>
          ))}
          {!customAdapters.length && !addingTracker ? <p className="adapter-empty">No additional trackers configured.</p> : null}
          {exampleAdapter ? (
            <details className="example-adapter">
              <summary>What is the example adapter?</summary>
              <p>It is developer documentation expressed as a working local adapter with synthetic—not real—usage data. It stays disabled and is not one of your additional trackers.</p>
              <code>{exampleAdapter.command}</code>
            </details>
          ) : null}
          <p className="adapter-warning">Adapters are local executable integrations, not provider accounts. Trust is bound to the manifest and executable hashes; any file change disables execution until you review and trust it again.</p>
        </section>

        <section className="settings-section">
          <div className="section-heading"><div><span>04</span><h2>General</h2></div><p>Refresh and startup behavior.</p></div>
          <div className="setting-row">
            <div><strong>Refresh interval</strong><small>Countdowns update locally between reads.</small></div>
            <select value={app.settings.refresh_interval_seconds} onChange={(event) => void update({ refresh_interval_seconds: Number(event.target.value) })}>
              <option value={60}>1 minute</option><option value={300}>5 minutes</option><option value={600}>10 minutes</option><option value={900}>15 minutes</option>
            </select>
          </div>
          <div className="setting-row">
            <div><strong>Start at login</strong><small>Uses Cinnamon’s normal per-user autostart entry.</small></div>
            <Switch checked={app.autostart_enabled} onChange={(value) => void run("set_autostart", { enabled: value })} label="Start at login" />
          </div>
          <div className="setting-row">
            <div><strong>Refresh all trackers</strong><small>Does not invoke a model or consume tokens.</small></div>
            <button onClick={() => void run("refresh_provider", { providerId: null })}>Refresh now</button>
          </div>
        </section>

        <section className="settings-section diagnostics">
          <div className="section-heading"><div><span>05</span><h2>Diagnostics</h2></div><p>Local paths and build information.</p></div>
          <dl><div><dt>Version</dt><dd>{app.app_version}</dd></div><div><dt>Config</dt><dd>{app.paths.config}</dd></div><div><dt>Cache</dt><dd>{app.paths.cache}</dd></div><div><dt>Adapters</dt><dd>{app.paths.adapters}</dd></div></dl>
          <p className="privacy-note">No telemetry · no hosted service · no provider credentials stored</p>
        </section>
      </div>

      <footer className="settings-footer">
        <span>{notice ?? (busy ? "Working…" : "Changes save automatically")}</span>
        <button onClick={() => void run("close_settings")}>Done</button>
      </footer>
    </main>
  );
}

function Switch({ checked, onChange, label }: { checked: boolean; onChange: (value: boolean) => void; label: string }) {
  return <label className="switch"><input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} aria-label={label} /><span /></label>;
}

function freshnessFor(provider: Provider, now: number) {
  if (provider.state === "error" || provider.state === "disconnected") return { tone: "error", glyph: "!", label: provider.state === "error" ? "Error" : "Disconnected" };
  if (provider.state === "waiting" || provider.state === "untrusted") return { tone: "waiting", glyph: "○", label: provider.state === "untrusted" ? "Untrusted" : "Waiting" };
  if (!provider.observed_at) return { tone: "waiting", glyph: "○", label: "Waiting" };
  const age = now - provider.observed_at;
  if (age > 1800) return { tone: "stale", glyph: "△", label: "Stale" };
  if (age > 600) return { tone: "aging", glyph: "·", label: "Updated " + relativeAge(age) };
  return { tone: "fresh", glyph: "●", label: "Updated " + relativeAge(age) };
}

function orderProviders(providers: Provider[], settings: Settings) {
  return [...providers].sort((left, right) => {
    const leftIndex = settings.provider_order.indexOf(left.id);
    const rightIndex = settings.provider_order.indexOf(right.id);
    return (leftIndex < 0 ? Number.MAX_SAFE_INTEGER : leftIndex) - (rightIndex < 0 ? Number.MAX_SAFE_INTEGER : rightIndex);
  });
}

function formatReset(timestamp: number | null, now: number) {
  if (!timestamp) return "Reset unavailable";
  const seconds = timestamp - now;
  if (seconds <= 0) return "Reset due";
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `Resets in ${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  if (hours < 48) return `Resets in ${hours}h${remainder ? ` ${remainder}m` : ""}`;
  return `Resets in ${Math.floor(hours / 24)}d ${hours % 24}h`;
}

function formatAbsolute(timestamp: number | null) {
  if (!timestamp) return "Time unavailable";
  return new Intl.DateTimeFormat(undefined, { weekday: "short", hour: "numeric", minute: "2-digit" }).format(new Date(timestamp * 1000));
}

function relativeAge(seconds: number) {
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  return `${Math.floor(seconds / 3600)}h ago`;
}

function formatBalance(balance: Balance) {
  if (!balance.known || balance.amount === null) return "—";
  if (balance.unit === "USD") return `$${balance.amount}`;
  return `${balance.amount} ${balance.unit ?? ""}`.trim();
}

function formatPercent(value: number) {
  return `${Number.isInteger(value) ? value.toFixed(0) : value.toFixed(1)}%`;
}

function clamp(value: number) { return Math.max(0, Math.min(100, value)); }
function title(value: string) { return value.charAt(0).toUpperCase() + value.slice(1); }

export default App;
