/* OwnNAS OCCT worker — keeps STEP/IGES tessellation off the UI thread. */
importScripts("/assets/vendor/occt/occt-import-js.js");

let occtPromise = null;

function getOcct() {
  if (!occtPromise) {
    occtPromise = occtimportjs({
      locateFile(path) {
        return `/assets/vendor/occt/${path}`;
      },
    });
  }
  return occtPromise;
}

onmessage = async function (ev) {
  const data = ev.data || {};
  try {
    const occt = await getOcct();
    const format = data.format || "step";
    const buffer = data.buffer;
    if (!(buffer instanceof Uint8Array)) {
      throw new Error("Missing STEP/IGES buffer");
    }
    const params = data.params === undefined ? null : data.params;
    const result = occt.ReadFile(format, buffer, params);
    postMessage({ ok: true, result });
  } catch (err) {
    postMessage({
      ok: false,
      error: err && err.message ? err.message : String(err || "OCCT import failed"),
    });
  }
};
