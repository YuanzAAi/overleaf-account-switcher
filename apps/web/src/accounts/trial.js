export function trialUsageMeta(account, nowSeconds = Date.now() / 1000) {
  if (!account || ["free", "unknown", "needs_refresh"].includes(account.subscription_status)) return null;
  const expiry = Number(account.trial_expiry);
  if (!Number.isFinite(expiry) || expiry <= 0) return null;
  const start = Number(account.trial_started_at);
  const hasPeriod = Number.isFinite(start) && start > 0 && start < expiry;
  const duration = expiry - start;
  const totalDays = hasPeriod ? Math.max(1, Math.round(duration / 86400)) : null;
  const elapsed = Math.max(0, nowSeconds - start);
  const remainingDays = Math.max(0, Math.ceil((expiry - nowSeconds) / 86400));
  return {
    totalDays,
    usedDays: hasPeriod ? Math.min(totalDays, Math.floor(elapsed / 86400) + 1) : null,
    remainingDays,
    percent: hasPeriod ? Math.min(100, elapsed / duration * 100) : null,
    expired: remainingDays <= 0 || String(account.subscription_status || "").toLowerCase().includes("expired"),
    expiry,
  };
}
