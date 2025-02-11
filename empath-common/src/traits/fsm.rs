pub trait FiniteStateMachine {
    type Input;
    type Context;

    #[must_use]
    fn transition(self, input: Self::Input, context: &mut Self::Context) -> Self;
}
