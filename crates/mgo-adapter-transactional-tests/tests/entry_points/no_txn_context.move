// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

//# init --addresses Test=0x0 --accounts A

//# publish
module Test::M {
    use mgo::tx_context::{Self, TxContext};
    struct Obj has key {
        id: mgo::object::UID,
        value: u64
    }

    public entry fun mint(ctx: &mut TxContext) {
        mgo::transfer::transfer(
            Obj { id: mgo::object::new(ctx), value: 0 },
            tx_context::sender(ctx),
        )
    }

    public entry fun incr(obj: &mut Obj) {
        obj.value = obj.value + 1
    }
}

//# run Test::M::mint --sender A

//# run Test::M::incr --sender A --args object(2,0)
