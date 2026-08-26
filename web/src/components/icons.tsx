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

export const CopyIcon = () => (
  <Icon>
    <rect x="9" y="9" width="11" height="11" />
    <path d="M15 5H5v10" />
  </Icon>
);

export const CheckIcon = () => (
  <Icon>
    <path d="M4 12.5 9.5 18 20 6" />
  </Icon>
);

export const Mark = () => (
  <svg width="28" height="28" viewBox="0 0 24 24" aria-hidden="true">
    {/* The cachet mark is a capacitor: two leads meeting two plates
        across a gap. A cache is the same shape of thing, which is the
        joke, and it only reads as one if the plates are the short pair
        and the leads are the long ones. */}
    <path
      d="M2 12 H9 M15 12 H22 M9 5 V19 M15 5 V19"
      stroke="currentColor"
      strokeWidth="2"
      fill="none"
    />
  </svg>
);
