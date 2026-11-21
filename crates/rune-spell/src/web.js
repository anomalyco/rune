export async function fetchBytes(url) {
    const response = await fetch(new URL(url, document.baseURI));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}
