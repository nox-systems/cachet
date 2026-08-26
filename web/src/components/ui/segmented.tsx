import * as stylex from "@stylexjs/stylex";

import {
  color,
  font,
  leading,
  space,
  text,
} from "../../styles/tokens.stylex.ts";

// A row of exclusive choices. The console's filters are closed enums the
// worker refuses anything outside of, so the control is a fixed set of
// buttons rather than a text field that could ask an unanswerable
// question.

const styles = stylex.create({
  group: {
    display: "inline-flex",
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.line,
  },
  option: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    paddingBlock: space.s2,
    paddingInline: space.s4,
    backgroundColor: "transparent",
    borderWidth: 0,
    borderLeftWidth: { default: "1px", ":first-child": 0 },
    borderLeftStyle: "solid",
    borderLeftColor: color.line,
    color: { default: color.muted, ":hover": color.text },
    cursor: "pointer",
    transitionProperty: "color, background-color",
    transitionDuration: "140ms",
  },
  selected: {
    color: color.ink,
    backgroundColor: color.text,
    ":hover": { color: color.ink },
  },
});

export const Segmented = <T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly { readonly value: T; readonly label: string }[];
  value: T;
  onChange: (next: T) => void;
}) => (
  <div {...stylex.props(styles.group)} role="group" aria-label={label}>
    {options.map((option) => (
      <button
        key={option.value}
        type="button"
        aria-pressed={option.value === value}
        onClick={() => onChange(option.value)}
        {...stylex.props(
          styles.option,
          option.value === value && styles.selected,
        )}
      >
        {option.label}
      </button>
    ))}
  </div>
);
