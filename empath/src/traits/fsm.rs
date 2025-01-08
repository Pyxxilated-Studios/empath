pub trait FiniteStateMachine {
    type Input;
    type Context;

    fn transition(self, input: Self::Input, context: &mut Self::Context) -> Self;
}
