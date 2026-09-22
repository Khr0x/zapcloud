// One SDK operation. The Python runner owns cases, assertions and cleanup.
import fs from "node:fs";
import * as lambda from "@aws-sdk/client-lambda";

const input = JSON.parse(fs.readFileSync(0, "utf8"));
const client = new lambda.LambdaClient({
  region: input.region,
  ...(input.endpoint ? { endpoint: input.endpoint } : {}),
  maxAttempts: 1,
});
let wire;
const handler = client.config.requestHandler;
const handle = handler.handle.bind(handler);
handler.handle = async (...args) => {
  const result = await handle(...args);
  wire = { status: result.response.statusCode, headers: result.response.headers };
  return result;
};
const params = input.params;
if (params.Code?.ZipFile) params.Code.ZipFile = Buffer.from(params.Code.ZipFile, "base64");
if (params.ZipFile) params.ZipFile = Buffer.from(params.ZipFile, "base64");
if (params.Payload) params.Payload = Buffer.from(params.Payload, "base64");
try {
  const result = await client.send(new lambda[`${input.operation}Command`](params));
  delete result.$metadata;
  const body = input.operation === "Invoke"
    ? JSON.parse(Buffer.from(result.Payload).toString("utf8")) : result;
  process.stdout.write(JSON.stringify({ ...wire, body }));
} catch (error) {
  // No HTTP response means a client/transport failure, never an expected AWS error.
  if (!wire) throw error;
  process.stdout.write(JSON.stringify({ ...wire, error: error.name, body: { message: error.message } }));
} finally {
  client.destroy();
}
