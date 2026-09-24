import { readAccountUsage, type AccountUsage } from "../shared/account-usage.js";
import { nativeAccountUsage } from "./provider.js";

/** Read only the registered engines' native quota observations. */
export async function getAccountUsage(): Promise<AccountUsage> {
  try {
    return readAccountUsage.output.parse({
      available: true,
      connections: await nativeAccountUsage(),
    });
  } catch {
    return { available: false, connections: [] };
  }
}
