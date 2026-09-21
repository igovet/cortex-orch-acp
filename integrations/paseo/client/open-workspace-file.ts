/** Paseo 0.8 gives plugins no file opener; the chat's "Open file" goes
 * through the host's own workspace deep link instead:
 * /h/<server>/workspace/<id>?open=file:<base64url path>. The route parses the
 * path against the workspace root and removes `open` once the tab is shown. */
// ponytail: mirrors Paseo's internal route format (workspace-route encoding);
// switch to navigation.openFile once the plugin API offers one.

function base64Url(text: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

export function workspaceFileRoute(serverId: string, workspaceId: string, path: string): string {
  const id = workspaceId.trim();
  const segment = /^[A-Za-z0-9._~-]+$/.test(id) ? id : `b64_${base64Url(id)}`;
  return `/h/${encodeURIComponent(serverId.trim())}/workspace/${encodeURIComponent(segment)}`
    + `?open=${encodeURIComponent(`file:${base64Url(path.trim())}`)}`;
}

/** Navigate like Back/Forward so the router, not a reload, handles the link. */
export function openWorkspaceFile(serverId: string, workspaceId: string, path: string): void {
  window.history.pushState(null, "", workspaceFileRoute(serverId, workspaceId, path));
  window.dispatchEvent(new PopStateEvent("popstate"));
}
