// SPDX-License-Identifier: MIT
(() => {
    const origin = 'https://tape.api.haus', CHUNK = 1024 * 1024;
    const utf8 = new TextEncoder();
    const hex = bytes => Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
    let destination;

    // The embedding app supplies the owner’s upload-only credential for this page lifetime.
    function authorize(ip, token, device = "") {
        if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(ip) || !/^[a-zA-Z0-9_-]{32,256}$/.test(token)) return false;
        const [a, b, ...tail] = ip.split('.').map(Number);
        if (tail.some(n => n > 255) || !(a === 10 && b <= 255 || a === 172 && b >= 16 && b <= 31 || a === 192 && b === 168)) return false;
        destination = { endpoint: 'https://' + [a, b, ...tail].join('.') + ':8443', token, device };
        return true;
    }

    function connect() {
        const dialog = document.createElement('dialog');
        dialog.innerHTML = '<form><h2>Connect to Frametape</h2><p><label>LAN address <input name="ip" required></label></p><p><label>Upload token <input name="token" type="password" required></label></p><p><label>Device name <input name="device" required></label></p><button type="submit">Connect</button> <button type="button">Cancel</button><p role="alert"></p></form>';
        const form = dialog.querySelector('form');
        form.addEventListener('submit', event => {
            event.preventDefault();
            const fields = new FormData(form);
            if (!authorize(fields.get('ip').trim(), fields.get('token').trim(), fields.get('device').trim())) {
                form.querySelector('[role="alert"]').textContent = 'Enter a private LAN IPv4 address and a valid upload token.';
                return;
            }
            dialog.close();
        });
        form.querySelector('[type="button"]').onclick = () => dialog.close();
        dialog.addEventListener('close', () => dialog.remove(), { once: true });
        document.body.append(dialog);
        dialog.showModal();
    }

    function begin(report = () => {}) {
        if (!destination) return null;
        const { endpoint, token: credential, device } = destination, id = crypto.randomUUID();
        const key = crypto.subtle.digest('SHA-256', utf8.encode(id)).then(b => hex(new Uint8Array(b, 0, 16)));
        const videoName = 'attachments/video.h264', video = [];
        const offsets = new Map();
        let timer, pumping = Promise.resolve(), stopped = false, videoBytes = 0, bytes = 0, confirmation;

        const publish = value => report(value);
        async function send(method, suffix, body) {
            const response = await fetch(endpoint + '/ingest/' + await key + '/' + suffix, {
                method, body, headers: { Authorization: 'Bearer ' + credential, 'Content-Type': 'application/octet-stream' },
                credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(method === 'POST' ? 120000 : 8000),
            });
            if (!response.ok) throw Error('Portal returned HTTP ' + response.status);
        }
        async function part(name, data) {
            const start = offsets.get(name) ?? 0;
            await send('PUT', hex(utf8.encode(name)) + '/' + start, data);
            const length = data instanceof Blob ? data.size : data.byteLength;
            offsets.set(name, start + length);
            bytes += length;
        }
        async function pump() {
            try {
                while (videoBytes && !stopped) {
                    const count = Math.min(videoBytes, CHUNK);
                    const parts = [];
                    let remain = count;
                    for (const segment of video) {
                        if (remain === 0) break;
                        parts.push(segment.subarray(0, remain));
                        remain -= parts.at(-1).length;
                    }
                    await part(videoName, new Blob(parts));
                    video.splice(0, parts.length - 1);
                    video[0] = video[0].subarray(parts.at(-1).length);
                    if (!video[0].length) video.shift();
                    videoBytes -= count;
                    publish({ state: 'streaming', bytes });
                }
            } catch (error) {
                publish({ state: 'paused', error: error.message });
            }
            if (!stopped) timer = setTimeout(() => { pumping = pump(); }, 500);
        }
        publish({ state: 'connecting' });
        timer = setTimeout(() => { pumping = pump(); }, 500);
        return {
            id, device,
            video(bytes) {
                if (!stopped && bytes.length) {
                    video.push(bytes);
                    videoBytes += bytes.length;
                }
            },
            abort() {
                stopped = true;
                clearTimeout(timer);
            },
            finish(members) {
                stopped = true;
                clearTimeout(timer);
                confirmation = (async () => {
                    await pumping;
                    video.length = 0;
                    publish({ state: 'finalizing' });
                    for (const [name, data] of members)
                        while ((offsets.get(name) ?? 0) < data.length) {
                            const start = offsets.get(name) ?? 0;
                            await part(name, data.subarray(start, start + CHUNK));
                        }
                    const inventory = [];
                    for (const [name, data] of members) inventory.push({
                        name, bytes: data.length, sha256: hex(new Uint8Array(await crypto.subtle.digest('SHA-256', data))),
                    });
                    await send('POST', 'commit', JSON.stringify(inventory));
                    const url = origin + '/?take=' + await key;
                    publish({ state: 'confirmed', url });
                    return url;
                })().catch(error => {
                    publish({ state: 'failed', error: error.message });
                    return null;
                });
            },
            get confirmation() { return confirmation; },
        };
    }

    function badge() {
        const element = document.createElement('div');
        element.setAttribute('role', 'status');
        element.style.cssText = 'position:fixed;top:12px;left:12px;z-index:1000;background:#141619;color:#fff;padding:5px 9px;border-radius:4px;font:600 13px system-ui';
        element.hidden = true;
        document.body.append(element);
        return {
            remove: () => element.remove(),
            show({ state, url }) {
                const labels = {
                    local: '● REC', connecting: '● REC · connecting', streaming: '● REC · streaming',
                    paused: '● REC · upload paused', finalizing: 'Uploading take…',
                    capture_failed: 'Recording failed', failed: 'Portal upload failed', confirmed: 'Take on portal',
                };
                element.hidden = !labels[state];
                if (url) {
                    const link = document.createElement('a');
                    link.textContent = labels[state];
                    link.href = url;
                    link.target = '_blank';
                    link.rel = 'noopener';
                    link.style.color = 'inherit';
                    element.replaceChildren(link);
                } else element.textContent = labels[state] ?? '';
            },
        };
    }

    globalThis.FrametapePortal = { authorize, begin, badge, connect };
})();
