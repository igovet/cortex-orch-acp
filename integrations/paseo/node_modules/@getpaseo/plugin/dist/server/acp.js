import { createAcpProviderConnection } from "./acp-internal/connection.js";
export function runAcpProvider(options) {
    const hasCommand = options.command !== undefined;
    const hasConnector = options.connector !== undefined;
    if (hasCommand === hasConnector) {
        throw new Error("ACP provider requires exactly one command or SDK connector");
    }
    if (options.command && (options.command.length === 0 || options.command[0].trim().length === 0)) {
        throw new Error("ACP provider command must contain an executable");
    }
    return {
        id: options.id,
        label: options.label,
        description: options.description,
        icon: options.icon,
        connect(request) {
            return createAcpProviderConnection(options, request);
        },
    };
}
//# sourceMappingURL=acp.js.map