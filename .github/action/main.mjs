import { runAction } from './run.mjs';

try {
  process.exitCode = await runAction();
} catch (error) {
  // Workflow commands require escaped data, including errors containing paths.
  const message = String(error.message ?? error)
    .replaceAll('%', '%25').replaceAll('\r', '%0D').replaceAll('\n', '%0A');
  console.error(`::error::${message}`);
  process.exitCode = 2;
}
