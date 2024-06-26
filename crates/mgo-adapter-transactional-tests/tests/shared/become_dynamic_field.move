// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

// tests that shared objects cannot become dynamic fields and that a shared object
// dynamic field added and removed in the same transaction does not error

//# init --addresses a=0x0 --accounts A --shared-object-deletion true

//# publish
module a::m {
    use mgo::transfer;
    use mgo::dynamic_field::{add, remove};
    use mgo::object;
    use mgo::tx_context::{sender, TxContext};

    struct Outer has key, store {
        id: object::UID,
    }

    struct Inner has key, store {
        id: object::UID,
    }

    public entry fun create_shared(ctx: &mut TxContext) {
        transfer::public_share_object(Inner { id: object::new(ctx) })
    }

    public entry fun add_dynamic_field(inner: Inner, ctx: &mut TxContext) {
        let outer = Outer {id: object::new(ctx)};
        add<u64, Inner>(&mut outer.id, 0, inner);
        transfer::transfer(outer, sender(ctx));
    }

    public entry fun add_and_remove_dynamic_field(inner: Inner, ctx: &mut TxContext) {
        let outer = Outer {id: object::new(ctx)};
        add<u64, Inner>(&mut outer.id, 0, inner);
        let removed = remove<u64, Inner>(&mut outer.id, 0);
        transfer::public_share_object(removed);
        transfer::transfer(outer, sender(ctx));
    }

}

//# run a::m::create_shared --sender A

//# view-object 2,0

//# run a::m::add_dynamic_field --sender A --args object(2,0)

//# run a::m::create_shared --sender A

//# view-object 5,0

//# run a::m::add_and_remove_dynamic_field --sender A --args object(5,0)
