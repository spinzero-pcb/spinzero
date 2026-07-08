import React from "react";
import { ipc } from "../lib/ipc";

interface Props {
  children: React.ReactNode;
}

interface State {
  error: Error | null;
}

/** Catches render/lifecycle errors anywhere below it so a single broken
 *  component shows a recoverable fallback instead of a blank white window.
 *  The error is forwarded to the backend log (same file the Rust side writes)
 *  so a crash report carries both halves of the picture. */
export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    void ipc.logError(
      `React render error: ${error.message}\n${error.stack ?? "(no stack)"}\n` +
        `Component stack:${info.componentStack ?? " (none)"}`,
    );
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div style={styles.overlay} role="alert">
        <div style={styles.card}>
          <div style={styles.title}>The app hit an unexpected error</div>
          <div style={styles.subtitle}>
            Your project data is safe on disk. You can try to recover the view, or
            reload the app.
          </div>
          <pre style={styles.detail}>{error.message}</pre>
          <div style={styles.actions}>
            <button
              style={{ ...styles.button, ...styles.primary }}
              onClick={() => window.location.reload()}
            >
              Reload app
            </button>
            <button
              style={styles.button}
              onClick={() => this.setState({ error: null })}
            >
              Try to recover
            </button>
          </div>
        </div>
      </div>
    );
  }
}

const styles: Record<string, React.CSSProperties> = {
  overlay: {
    position: "fixed",
    inset: 0,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: "var(--bg-0, #0e1116)",
    padding: 24,
    zIndex: 9999,
    fontFamily: "var(--font-ui, system-ui, sans-serif)",
  },
  card: {
    maxWidth: 560,
    width: "100%",
    background: "var(--bg-1, #14181f)",
    border: "1px solid var(--border-1, #333c4b)",
    borderLeft: "3px solid var(--err, #f85149)",
    borderRadius: "var(--radius-2, 6px)",
    padding: 24,
  },
  title: {
    fontSize: 16,
    fontWeight: 600,
    color: "var(--fg-0, #e8ebf0)",
    marginBottom: 8,
  },
  subtitle: {
    fontSize: 13,
    color: "var(--fg-1, #a9b2c0)",
    marginBottom: 16,
    lineHeight: 1.5,
  },
  detail: {
    fontFamily: "var(--font-mono, monospace)",
    fontSize: 12,
    color: "var(--fg-1, #a9b2c0)",
    background: "var(--bg-2, #1b212b)",
    border: "1px solid var(--border-0, #262d39)",
    borderRadius: "var(--radius-1, 4px)",
    padding: 12,
    maxHeight: 160,
    overflow: "auto",
    whiteSpace: "pre-wrap",
    wordBreak: "break-word",
    marginBottom: 16,
  },
  actions: { display: "flex", gap: 8 },
  button: {
    fontFamily: "inherit",
    fontSize: 13,
    padding: "8px 14px",
    borderRadius: "var(--radius-1, 4px)",
    border: "1px solid var(--border-1, #333c4b)",
    background: "var(--bg-2, #1b212b)",
    color: "var(--fg-0, #e8ebf0)",
    cursor: "pointer",
  },
  primary: {
    background: "var(--accent, #4f8cff)",
    borderColor: "var(--accent, #4f8cff)",
    color: "var(--accent-fg, #ffffff)",
  },
};
