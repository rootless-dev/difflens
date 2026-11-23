import express, { Request, Response } from "express";

import { requireUser } from "./middleware";
import { usersRouter } from "./routes/users";

const app = express();

app.use(express.json());
app.use(requireUser);
app.use("/api/users", usersRouter);

app.get("/health", health);

app.post("/echo", (req: Request, res: Response) => {
  res.json(req.body);
});

function health(_req: Request, res: Response): void {
  res.send("ok");
}

export function start(port: number): void {
  app.listen(port, () => {
    console.log(`listening on ${port}`);
  });
}

start(3000);
