// SPDX-License-Identifier: MIT
// Host supplies recording clock and frame ownership through publish(data, sourceTimestampMs).
globalThis.FrametapeInput = {
    attach({ target = window, publish, enabled = () => true, keyboard = true, pointer = true, touch = true, text = true, focus = true, gamepad = true }) {
        if (typeof publish !== 'function') throw new TypeError('publish callback required');
        const kinds = [
            ...(keyboard ? ['keydown', 'keyup', 'keypress'] : []),
            ...(pointer ? ['pointerdown', 'pointerup', 'pointermove', 'pointercancel', 'mousedown', 'mouseup', 'mousemove', 'wheel'] : []),
            ...(touch ? ['touchstart', 'touchmove', 'touchend', 'touchcancel'] : []),
            ...(text ? ['beforeinput', 'input', 'compositionstart', 'compositionupdate', 'compositionend'] : []),
            ...(focus ? ['blur', 'focus'] : []),
        ];
        let previousPads = '';
        const input = (e) => {
            if (!enabled()) return;
            const data = { kind: e.type };
            for (const key of ['key', 'code', 'repeat', 'ctrlKey', 'altKey', 'shiftKey', 'metaKey', 'button', 'buttons', 'pointerId', 'pointerType', 'clientX', 'clientY', 'movementX', 'movementY', 'pressure', 'deltaX', 'deltaY', 'deltaZ', 'deltaMode', 'data', 'inputType']) if (e[key] !== undefined) data[key] = e[key];
            if (e.changedTouches) data.touches = Array.from(e.changedTouches, (t) => ({ id: t.identifier, x: t.clientX, y: t.clientY, force: t.force }));
            publish(data, e.timeStamp);
        };
        for (const kind of kinds) target.addEventListener(kind, input, { capture: true, passive: true });
        return {
            pollGamepads() {
                if (!gamepad || !enabled() || !navigator.getGamepads) return;
                const pads = Array.from(navigator.getGamepads(), (p) => p && ({ id: p.id, index: p.index, axes: Array.from(p.axes), buttons: p.buttons.map((b) => ({ value: b.value, pressed: b.pressed, touched: b.touched })) }));
                const json = JSON.stringify(pads);
                if (json !== previousPads) { previousPads = json; publish({ kind: 'gamepad', pads }, performance.now()); }
            },
            reset() { previousPads = ''; },
            dispose() { for (const kind of kinds) target.removeEventListener(kind, input, true); },
        };
    },
};
