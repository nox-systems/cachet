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

/** The crosshair the rail's foot carries: the same motif as the nox
 *  mark's centre, at rail scale. */
export const StatusIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 1.5 V5.5 M12 18.5 V22.5 M1.5 12 H5.5 M18.5 12 H22.5" />
  </Icon>
);

/** The nox wordmark: N, a crosshaired O with a live centre, X.
 *
 * Taken from the design as drawn. The N is a filled outline rather than a
 * stroked path, which is what makes it clean at this size: a stroked N
 * has two acute joins, and every join setting trades one artifact for
 * another. A filled glyph has no joins.
 *
 * The centre dot keeps its literal red. It is the one mark in the console
 * that is the brand's colour rather than the theme's, and it stays that
 * colour wherever the wordmark is drawn.
 */
export const NoxMark = () => (
  <svg width="48" height="16" viewBox="0 0 72 24" aria-hidden="true">
    <path d="M2 2 H6 L18 16 V2 H22 V22 H18 L6 8 V22 H2 Z" fill="currentColor" />
    <circle
      cx="36"
      cy="12"
      r="8"
      fill="none"
      stroke="currentColor"
      strokeWidth="4"
    />
    <path
      d="M36 0 V5 M36 19 V24 M24 12 H29 M43 12 H48"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
    />
    <circle cx="36" cy="12" r="1.6" fill="#E4002B" />
    <path
      d="M52 2 L68 22 M68 2 L52 22"
      fill="none"
      stroke="currentColor"
      strokeWidth="4"
    />
  </svg>
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
