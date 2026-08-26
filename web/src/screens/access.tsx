import { useQuery } from "@tanstack/react-query";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import * as api from "../api/client.ts";
import { Label, Panel } from "../components/ui/primitives.tsx";
import { CheckIcon, CopyIcon } from "../components/icons.tsx";
import { Failed, LoadingScreen } from "../components/ui/states.tsx";
import { color, font, leading, space, text } from "../styles/tokens.stylex.ts";

// Who can reach this deployment and how. The one screen an org member
// who is not an admin can read, because everything on it is either
// public configuration or a fact about the reader's own credential.

const styles = stylex.create({
  rows: { display: "flex", flexDirection: "column" },
  row: {
    display: "grid",
    gridTemplateColumns: "20ch 1fr auto",
    gap: space.s6,
    // why: centred. A label beside a one-line value reads as one row
    // only when the two share a centre line; aligning both to the top
    // left the label riding above its own value.
    alignItems: "center",
    paddingBlock: space.s4,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
  },
  // The organizations row carries a second, explanatory line, so its
  // label belongs beside the first line rather than the block's middle.
  rowTop: { alignItems: "start" },
  value: {
    fontFamily: font.mono,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.text,
    wordBreak: "break-all",
  },
  note: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.muted,
    marginTop: space.s1,
  },
  copy: {
    fontFamily: font.ui,
    fontSize: text.label,
    letterSpacing: "0.08em",
    textTransform: "uppercase",
    color: { default: color.muted, ":hover": color.text },
    backgroundColor: "transparent",
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.line,
    paddingBlock: space.s2,
    paddingInline: space.s3,
    cursor: "pointer",
    flexShrink: 0,
  },
  columns: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(32ch, 1fr))",
    gap: space.s6,
    alignItems: "start",
  },
  code: {
    fontFamily: font.mono,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.text,
    backgroundColor: color.ink,
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.line,
    padding: space.s4,
    margin: 0,
    overflowX: "auto",
    whiteSpace: "pre",
  },
  steps: {
    display: "flex",
    flexDirection: "column",
    gap: space.s4,
    listStyle: "none",
    padding: 0,
    margin: 0,
  },
  step: { display: "flex", gap: space.s3, alignItems: "baseline" },
  stepNumber: {
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    color: color.muted,
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.line,
    paddingBlock: space.s1,
    paddingInline: space.s2,
    flexShrink: 0,
  },
  stepBody: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.text,
  },
  command: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: space.s4,
    // why: sized to the command, not to the column. A one-word command in
    // a full-width box reads as an input field waiting for the rest of it.
    alignSelf: "flex-start",
    maxWidth: "100%",
    textAlign: "left",
    cursor: "pointer",
    backgroundColor: { default: color.ink, ":hover": color.ink3 },
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: { default: color.line, ":hover": color.lineStrong },
    padding: space.s3,
    marginTop: space.s2,
    transitionProperty: "background-color, border-color",
    transitionDuration: "140ms",
  },
  commandText: {
    fontFamily: font.mono,
    fontSize: text.spec,
    color: color.text,
    overflowX: "auto",
    whiteSpace: "pre",
  },
  commandHint: {
    display: "flex",
    alignItems: "center",
    color: color.muted,
    flexShrink: 0,
  },
  copied: { color: color.text },
});

const useCopy = (value: string): [boolean, () => void] => {
  const [copied, setCopied] = useState(false);
  return [
    copied,
    () => {
      void navigator.clipboard?.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_400);
    },
  ];
};

/// A command, with the one affordance a command wants.
///
/// The whole block is the button and the mark is the hint, because the
/// thing being copied is right there and a word beside it would be read
/// as part of the command.
const Command = ({ children }: { children: string }) => {
  const [copied, copy] = useCopy(children);
  return (
    <button
      type="button"
      onClick={copy}
      aria-label={copied ? "Copied" : `Copy: ${children}`}
      {...stylex.props(styles.command)}
    >
      <code {...stylex.props(styles.commandText)}>{children}</code>
      <span {...stylex.props(styles.commandHint, copied && styles.copied)}>
        {copied ? <CheckIcon /> : <CopyIcon />}
      </span>
    </button>
  );
};

const Copy = ({ value }: { value: string }) => {
  const [copied, copy] = useCopy(value);
  return (
    <button type="button" {...stylex.props(styles.copy)} onClick={copy}>
      {copied ? "Copied" : "Copy"}
    </button>
  );
};

export const Access = () => {
  const config = useQuery({
    queryKey: ["config"],
    queryFn: api.getConfig,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  if (config.isPending) return <LoadingScreen />;
  if (config.error !== null) return <Failed message={String(config.error)} />;

  const deployment = config.data;
  const url = `https://${deployment.host}`;
  const action = `
permissions:
  contents: read
  id-token: write

steps:
  - uses: actions/checkout@v7
  - uses: nox-systems/cachet/action@v0
    with:
      cache-url: ${url}
      roots: |
        .#my-package
`.trim();

  return (
    <>
      <Panel title="Who has access">
        <div {...stylex.props(styles.rows)}>
          <div {...stylex.props(styles.row)}>
            <Label>Organizations</Label>
            <span {...stylex.props(styles.value)}>
              {deployment.orgs.join(", ")}
            </span>
            <span />
          </div>

          <div {...stylex.props(styles.row)}>
            <Label>Public key</Label>
            <span {...stylex.props(styles.value)}>{deployment.publicKey}</span>
            <Copy value={deployment.publicKey} />
          </div>

          <div {...stylex.props(styles.row)}>
            <Label>OAuth client</Label>
            <span {...stylex.props(styles.value)}>
              {deployment.oauthClientId}
            </span>
            <span />
          </div>
        </div>
      </Panel>

      <div {...stylex.props(styles.columns)}>
        <Panel title="Push from CI" aside={<Copy value={action} />}>
          <pre {...stylex.props(styles.code)}>{action}</pre>
        </Panel>

        <Panel title="Read from a laptop">
          <ol {...stylex.props(styles.steps)}>
            <li {...stylex.props(styles.step)}>
              <span {...stylex.props(styles.stepNumber)}>1</span>
              <span {...stylex.props(styles.stepBody)}>
                Sign in through GitHub.
                <Command>{`cachet login --cache-url ${url}`}</Command>
              </span>
            </li>
            <li {...stylex.props(styles.step)}>
              <span {...stylex.props(styles.stepNumber)}>2</span>
              <span {...stylex.props(styles.stepBody)}>
                Enter the code it prints at github.com/login/device.
              </span>
            </li>
            <li {...stylex.props(styles.step)}>
              <span {...stylex.props(styles.stepNumber)}>3</span>
              <span {...stylex.props(styles.stepBody)}>
                Add the cache and its public key to nix.conf.
                <Command>cachet setup</Command>
              </span>
            </li>
          </ol>
        </Panel>
      </div>
    </>
  );
};
