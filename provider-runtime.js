import { ProviderSnapshotManager, } from "./provider-snapshot-manager.js";
import { OpenCodeBridge } from "./providers/opencode/bridge.js";
export async function createAgentProviderRuntime(options) {
    const bridge = new OpenCodeBridge({ paseoHome: options.paseoHome, logger: options.logger });
    try {
        await bridge.start();
        const snapshotManager = new ProviderSnapshotManager({
            ...options.snapshotManager,
            logger: options.logger.child({ module: "provider-snapshot-manager" }),
            openCodeBridge: bridge,
        });
        let shutdownPromise = null;
        return {
            snapshotManager,
            setPaseoToolCatalog: (catalog) => bridge.setManifestCatalog(catalog),
            shutdown: () => {
                shutdownPromise ?? (shutdownPromise = shutdownProviderRuntime(snapshotManager, bridge));
                return shutdownPromise;
            },
        };
    }
    catch (error) {
        await bridge.close().catch(() => undefined);
        throw error;
    }
}
async function shutdownProviderRuntime(snapshotManager, bridge) {
    try {
        await snapshotManager.shutdown();
    }
    finally {
        await bridge.close();
    }
}
//# sourceMappingURL=provider-runtime.js.map