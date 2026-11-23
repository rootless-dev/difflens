export interface User {
  id: string;
  name: string;
}

export class UserService {
  private readonly users = new Map<string, User>();

  async find(id: string): Promise<User | undefined> {
    return this.users.get(id);
  }

  create(name: string): User {
    const user = { id: String(this.users.size + 1), name };
    this.users.set(user.id, user);
    return user;
  }
}
