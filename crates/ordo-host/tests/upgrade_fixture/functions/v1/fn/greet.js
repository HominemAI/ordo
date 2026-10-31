// greet in version 1, hello in version 1.1: the same code under a new name.
export default {
  call(input, ctx) {
    return { name: ctx.function };
  },
};
