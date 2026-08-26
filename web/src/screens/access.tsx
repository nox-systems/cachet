import { useQuery } from "@tanstack/react-query";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import * as api from "../api/client.ts";
import { Label, Panel, Prose } from "../components/ui/primitives.tsx";
import { Failed, LoadingScreen } from "../components/ui/states.tsx";
import * as format from "../lib/format.ts";
import {
  color,
  font,
  leading,
  space,
  text,
  weight,
} from "../styles/tokens.stylex.ts";

// Who can reach this deployment and how. The one screen an org member
// who is not an admin can read, because everything on it is either
// public configuration or a fact about the reader's own credential.

const styles = stylex.create({
  rows: { display: "flex", flexDirection: "column" },
  row: {
    display: "grid",
    gridTemplateColumns: "20ch 1fr auto",
    gap: space.s6,
    alignItems: "start",
    paddingBlock: space.s4,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
  },
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
    display: "block",
    fontFamily: font.mono,
    color: color.text,
    backgroundColor: color.ink,
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.line,
    padding: space.s3,
    marginTop: space.s2,
    overflowX: "auto",
  },
  session: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.muted,
  },
  strong: { color: color.text, fontWeight: weight.bold },
});

const Copy = ({ value }: { value: string }) => {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      {...stylex.props(styles.copy)}
      onClick={() => {
        void navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1_400);
      }}
    >
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
  const who = useQuery({
    queryKey: ["whoami"],
    queryFn: api.getWhoAmI,
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
      <Panel title="Who has access" aside="Set in the deploy config">
        <div {...stylex.props(styles.rows)}>
          <div {...stylex.props(styles.row)}>
            <Label>Organizations</Label>
            <div>
              <span {...stylex.props(styles.value)}>
                {deployment.orgs.join(", ")}
              </span>
              <p {...stylex.props(styles.note)}>
                Members can read. CI in these organizations&apos; repositories
                can also write, through GitHub OIDC.
              </p>
            </div>
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
          <Prose>
            The action installs nix trusting this cache, snapshots the store
            before your build, and pushes what the build added on success.
            Signing happens on the deployment, so the job needs no secrets
            beyond its OIDC token.
          </Prose>
        </Panel>

        <Panel title="Read from a laptop" aside="Tokens last thirty days">
          <ol {...stylex.props(styles.steps)}>
            <li {...stylex.props(styles.step)}>
              <span {...stylex.props(styles.stepNumber)}>1</span>
              <span {...stylex.props(styles.stepBody)}>
                Sign in through GitHub.
                <code {...stylex.props(styles.command)}>
                  cachet login --cache-url {url}
                </code>
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
                <code {...stylex.props(styles.command)}>cachet setup</code>
              </span>
            </li>
          </ol>
          <Prose>
            Run cachet doctor to check the wiring, and cachet logout to revoke
            the credential before it expires. Laptops can only read; every path
            in the cache came from CI.
          </Prose>
        </Panel>
      </div>

      <p {...stylex.props(styles.session)}>
        {who.data === undefined ? (
          "You are not signed in to this console."
        ) : (
          <>
            You signed in through GitHub as{" "}
            <span {...stylex.props(styles.strong)}>{who.data.login}</span>
            {who.data.admin ? ", an admin of this deployment" : ""}.
            {who.data.expiresAtMs === undefined
              ? ""
              : ` This browser session expires on ${format.date(
                  who.data.expiresAtMs,
                )}.`}{" "}
            A browser session reads this console and nothing else: it cannot
            substitute from the cache.
          </>
        )}
      </p>
    </>
  );
};
