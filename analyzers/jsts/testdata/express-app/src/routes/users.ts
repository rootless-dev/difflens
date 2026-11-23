import { Router, Request, Response } from "express";

import { UserService } from "../users/service";

const service = new UserService();

export const usersRouter = Router();

usersRouter.get("/:id", getUser);
usersRouter.post("/", validate, createUser);
usersRouter.route("/:id/avatar").get(getAvatar).delete(deleteAvatar);

async function getUser(req: Request, res: Response): Promise<void> {
  const user = await service.find(req.params.id);
  res.json(user);
}

function validate(req: Request, res: Response, next: () => void): void {
  if (!req.body?.name) {
    res.status(400).end();
    return;
  }
  next();
}

function createUser(req: Request, res: Response): void {
  res.status(201).json(service.create(req.body.name));
}

function getAvatar(_req: Request, res: Response): void {
  res.sendStatus(404);
}

function deleteAvatar(_req: Request, res: Response): void {
  res.sendStatus(204);
}
