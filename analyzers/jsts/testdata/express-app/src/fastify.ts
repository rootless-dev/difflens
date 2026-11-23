// Fastify is not installed in this fixture: routes are recognised from the code alone.
import Fastify from "fastify";

const server = Fastify({ logger: false });

server.get("/ping", async () => ({ pong: true }));

server.route({
  method: "POST",
  url: "/items",
  handler: createItem,
});

server.register(itemsPlugin, { prefix: "/v2" });

async function createItem(): Promise<{ ok: boolean }> {
  return { ok: true };
}

async function itemsPlugin(instance: typeof server): Promise<void> {
  instance.get("/items/:id", listItem);
}

async function listItem(): Promise<string> {
  return "item";
}
