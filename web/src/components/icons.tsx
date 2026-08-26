import type { ReactNode } from "react";

// The rail's marks. Drawn at 18px on a 24px grid with a 1.5px stroke, so
// they read as one family rather than as five borrowed glyphs.

const Icon = ({ children }: { children: ReactNode }) => (
  <svg
    width="18"
    height="18"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.5"
    strokeLinecap="square"
    strokeLinejoin="miter"
    aria-hidden="true"
  >
    {children}
  </svg>
);

export const OverviewIcon = () => (
  <Icon>
    <rect x="3" y="3" width="7.5" height="7.5" />
    <rect x="13.5" y="3" width="7.5" height="7.5" />
    <rect x="3" y="13.5" width="7.5" height="7.5" />
    <rect x="13.5" y="13.5" width="7.5" height="7.5" />
  </Icon>
);

export const CollectionIcon = () => (
  <Icon>
    <path d="M20 12a8 8 0 1 1-2.4-5.7" />
    <path d="M20 3v5h-5" />
  </Icon>
);

export const AccessIcon = () => (
  <Icon>
    <path d="M3 15l5-6 4 4 4-6 5 5" />
  </Icon>
);

export const TrafficIcon = () => (
  <Icon>
    <path d="M4 20V11M9.3 20V4M14.7 20v-6M20 20V8" />
  </Icon>
);

export const LaptopIcon = () => (
  <Icon>
    <rect x="4" y="5" width="16" height="11" />
    <path d="M2 19h20" />
  </Icon>
);

export const SignOutIcon = () => (
  <Icon>
    <path d="M14 4H5v16h9" />
    <path d="M18 12H9M15 8l4 4-4 4" />
  </Icon>
);

export const Mark = () => (
  <svg width="28" height="28" viewBox="0 0 28 28" aria-hidden="true">
    {/* The cachet mark: two rules and a bar, the same shape the brand
        uses at every size. */}
    <path
      d="M4 8v12M24 8v12M4 14h20"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="square"
      fill="none"
    />
  </svg>
);
