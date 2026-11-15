//! Go 1.27 stable/symMerge ordering, including non-total NaN comparisons.
// Rust adaptation of Go src/sort/zsortinterface.go.
// Copyright 2022 The Go Authors. All rights reserved.
// Redistribution terms are in GO-SORT-LICENSE in this directory.
fn insertion<T>(data: &mut [T], a: usize, b: usize, less: &impl Fn(&T, &T) -> bool) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && less(&data[j], &data[j - 1]) {
            data.swap(j, j - 1);
            j -= 1;
        }
    }
}
fn swap_range<T>(data: &mut [T], a: usize, b: usize, n: usize) {
    for i in 0..n {
        data.swap(a + i, b + i);
    }
}
fn rotate<T>(data: &mut [T], a: usize, m: usize, b: usize) {
    let mut i = m - a;
    let mut j = b - m;
    while i != j {
        if i > j {
            swap_range(data, m - i, m, j);
            i -= j;
        } else {
            swap_range(data, m - i, m + j - i, i);
            j -= i;
        }
    }
    swap_range(data, m - i, m, i);
}
fn sym_merge<T>(data: &mut [T], a: usize, m: usize, b: usize, less: &impl Fn(&T, &T) -> bool) {
    let mut work = vec![(a, m, b)];
    while let Some((a, m, b)) = work.pop() {
        if m - a == 1 {
            let (mut i, mut j) = (m, b);
            while i < j {
                let h = (i + j) / 2;
                if less(&data[h], &data[a]) {
                    i = h + 1;
                } else {
                    j = h;
                }
            }
            for k in a..i - 1 {
                data.swap(k, k + 1);
            }
            continue;
        }
        if b - m == 1 {
            let (mut i, mut j) = (a, m);
            while i < j {
                let h = (i + j) / 2;
                if !less(&data[m], &data[h]) {
                    i = h + 1;
                } else {
                    j = h;
                }
            }
            for k in (i + 1..=m).rev() {
                data.swap(k, k - 1);
            }
            continue;
        }
        let mid = (a + b) / 2;
        let n = mid + m;
        let (mut start, mut r) = if m > mid { (n - b, mid) } else { (a, m) };
        let p = n - 1;
        while start < r {
            let c = (start + r) / 2;
            if !less(&data[p - c], &data[c]) {
                start = c + 1;
            } else {
                r = c;
            }
        }
        let end = n - start;
        if start < m && m < end {
            rotate(data, start, m, end);
        }
        // Preserve the reference's recursive left merge before its right merge.
        if mid < end && end < b {
            work.push((mid, end, b));
        }
        if a < start && start < mid {
            work.push((a, start, mid));
        }
    }
}
pub(crate) fn stable<T>(data: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let n = data.len();
    let mut block = 20;
    let (mut a, mut b) = (0, block);
    while b <= n {
        insertion(data, a, b, &less);
        a = b;
        b += block;
    }
    insertion(data, a, n, &less);
    while block < n {
        let (mut a, mut b) = (0, 2 * block);
        while b <= n {
            sym_merge(data, a, a + block, b, &less);
            a = b;
            b += 2 * block;
        }
        let m = a + block;
        if m < n {
            sym_merge(data, a, m, n, &less);
        }
        block *= 2;
    }
}
