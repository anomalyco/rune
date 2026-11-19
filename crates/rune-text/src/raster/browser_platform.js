// Browser platform and touch-point probe for `FontSmoothing`'s web defaults and text gamma.
export function browserPlatform() {
    const nav = globalThis.navigator;
    return nav ? (nav.userAgentData?.platform || nav.platform || '') : undefined;
}

export function touchPoints() {
    return globalThis.navigator?.maxTouchPoints || 0;
}
