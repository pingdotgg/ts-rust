delete process.env.HOME;

const environment: NodeJS.ProcessEnv = { ...process.env };
delete environment.GIT_DIR;

export {};
