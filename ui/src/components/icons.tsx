// Small inline icon set (stroke icons, currentColor).
import type { SVGProps } from "react";

const base = (p: SVGProps<SVGSVGElement>) => ({
  width: 16, height: 16, viewBox: "0 0 16 16", fill: "none", stroke: "currentColor",
  strokeWidth: 1.5, strokeLinecap: "round" as const, strokeLinejoin: "round" as const, ...p,
});

export const Plus = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M8 3v10M3 8h10" /></svg>;
export const Close = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M4 4l8 8M12 4l-8 8" /></svg>;
export const SplitRight = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M8 3v10" /></svg>;
export const SplitDown = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M2 8h12" /></svg>;
export const Zoom = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M3 6V3h3M13 6V3h-3M3 10v3h3M13 10v3h-3" /></svg>;
export const Search = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><circle cx="7" cy="7" r="4" /><path d="M10 10l3.5 3.5" /></svg>;
export const Chevron = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M6 4l4 4-4 4" /></svg>;
export const Server = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><rect x="2.5" y="2.5" width="11" height="4.5" rx="1" /><rect x="2.5" y="9" width="11" height="4.5" rx="1" /><path d="M5 4.75h.01M5 11.25h.01" /></svg>;
export const Folder = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M2 4.5a1 1 0 011-1h3l1.5 1.5H13a1 1 0 011 1V12a1 1 0 01-1 1H3a1 1 0 01-1-1z" /></svg>;
export const Terminal = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M5 7l2 1.5L5 10M8.5 10.5H11" /></svg>;
export const Layers = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M8 2.5l5.5 3L8 8.5l-5.5-3zM2.5 8.5L8 11.5l5.5-3M2.5 11L8 14l5.5-3" /></svg>;
export const Refresh = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M13 8a5 5 0 11-1.5-3.5M13 3v2.5h-2.5" /></svg>;
export const Gear = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><circle cx="8" cy="8" r="2" /><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" /></svg>;
export const Unplug = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M3 13l2.5-2.5M10.5 5.5L13 3M6 7l3 3M5 8.5L7.5 6 10 8.5 7.5 11z" /></svg>;
export const Plug = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M6 2.5v3M10 2.5v3M4.5 5.5h7v2.5a3.5 3.5 0 01-7 0zM8 11.5v2" /></svg>;
export const External = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M9 3h4v4M13 3L7.5 8.5M11.5 9.5v3a1 1 0 01-1 1h-7a1 1 0 01-1-1v-7a1 1 0 011-1h3" /></svg>;
export const Copy = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><rect x="5.5" y="5.5" width="8" height="8" rx="1.5" /><path d="M10.5 5.5V3.5a1 1 0 00-1-1h-6a1 1 0 00-1 1v6a1 1 0 001 1h2" /></svg>;
export const Pin = (p: SVGProps<SVGSVGElement>) => <svg {...base(p)}><path d="M9.5 2.5l4 4-2 1-2.5 2.5.5 3-1.5 1.5-6-6L3.5 7l3 .5L9 5z" /><path d="M5 11l-2.5 2.5" /></svg>;
