import type { DaemonTransport, DaemonTransportFactory, TransportLogger } from "./daemon-client-transport-types.js";
export declare function createRelayE2eeTransportFactory(args: {
    baseFactory: DaemonTransportFactory;
    daemonPublicKeyB64: string;
    logger: TransportLogger;
}): DaemonTransportFactory;
export declare function createEncryptedTransport(base: DaemonTransport, daemonPublicKeyB64: string, logger: TransportLogger): DaemonTransport;
//# sourceMappingURL=daemon-client-relay-e2ee-transport.d.ts.map