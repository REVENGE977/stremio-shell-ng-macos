let isTransparent = false;
let isApplying = false;
const modifiedElements = new Set<HTMLElement>();
let observer: MutationObserver | null = null;
let pollTimer: any = null;

export function isPlayerRoute(): boolean {
    const hash = window.location.hash || '';
    if (hash.startsWith('#/player') || hash.includes('/player/')) return true;
    if (document.querySelector('[class*="player-container"]') !== null) return true;
    if (document.querySelector('[class*="video-container"]') !== null) return true;
    return false;
}

function setElementTransparent(el: HTMLElement): void {
    // Check background-color longhand (and dataset marker) instead of the 'background'
    // shorthand: in WebKit (WKWebView on macOS), getPropertyValue('background') returns
    // '' or an expanded shorthand string rather than 'transparent', which previously
    // caused setProperty() to run on every MutationObserver turn in an infinite microtask loop.
    if (
        el.dataset.enhancedTransparent === '1' &&
        el.style.getPropertyValue('background-color') === 'transparent'
    ) {
        return;
    }
    el.dataset.enhancedTransparent = '1';
    el.style.setProperty('background', 'transparent', 'important');
    el.style.setProperty('background-color', 'transparent', 'important');
    modifiedElements.add(el);
}

function clearElementTransparent(el: HTMLElement): void {
    try {
        delete el.dataset.enhancedTransparent;
        el.style.removeProperty('background');
        el.style.removeProperty('background-color');
    } catch (_) {}
}

export function enforceTransparency(force?: boolean): void {
    if (isApplying) return;
    isApplying = true;
    try {
        const shouldBeTransparent = force !== undefined ? force : isPlayerRoute();
        const html = document.documentElement;
        const body = document.body;
        const app = document.getElementById('app');
        const playerContainer = document.querySelector<HTMLElement>('[class*="player-container"]');

        if (shouldBeTransparent) {
            if (!html || !body) return;
            isTransparent = true;

            setElementTransparent(html);
            setElementTransparent(body);

            if (app) {
                setElementTransparent(app);
            }

            if (playerContainer) {
                setElementTransparent(playerContainer);

                let curr = playerContainer.parentElement;
                while (curr && curr !== body && curr !== html) {
                    setElementTransparent(curr);
                    curr = curr.parentElement;
                }
            }
        } else if (isTransparent) {
            isTransparent = false;
            modifiedElements.forEach(el => clearElementTransparent(el));
            modifiedElements.clear();

            if (html) clearElementTransparent(html);
            if (body) clearElementTransparent(body);
            if (app) clearElementTransparent(app);
        }
    } finally {
        isApplying = false;
    }
}

export function setupTransparencyBridge(): void {
    enforceTransparency();

    window.addEventListener('hashchange', () => enforceTransparency());
    window.addEventListener('popstate', () => enforceTransparency());

    const attachObserver = () => {
        if (observer) return;
        const target = document.documentElement || document.body;
        if (!target) return;

        // Only observe structural/class changes (NOT 'style'), so our own inline style
        // updates never re-trigger the MutationObserver in a microtask loop on WebKit.
        observer = new MutationObserver(() => {
            enforceTransparency();
        });

        observer.observe(target, {
            childList: true,
            subtree: true,
            attributes: true,
            attributeFilter: ['class']
        });
    };

    if (document.documentElement || document.body) {
        attachObserver();
    } else {
        document.addEventListener('DOMContentLoaded', attachObserver, { once: true });
    }

    if (!pollTimer) {
        pollTimer = setInterval(() => {
            if (isPlayerRoute()) {
                enforceTransparency(true);
            } else if (isTransparent) {
                enforceTransparency(false);
            }
        }, 300);
    }
}
