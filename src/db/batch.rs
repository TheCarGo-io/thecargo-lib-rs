use std::collections::HashMap;
use std::hash::Hash;

pub fn rounds<K: Eq + Hash>(keys: impl IntoIterator<Item = K>) -> Vec<Vec<usize>> {
    let mut seen: HashMap<K, usize> = HashMap::new();
    let mut rounds: Vec<Vec<usize>> = Vec::new();
    for (index, key) in keys.into_iter().enumerate() {
        let round = seen.entry(key).or_insert(0);
        if rounds.len() == *round {
            rounds.push(Vec::new());
        }
        rounds[*round].push(index);
        *round += 1;
    }
    rounds
}

pub fn by_flag(round: &[usize], flag: impl Fn(usize) -> bool) -> [(bool, Vec<usize>); 2] {
    let (set, unset): (Vec<usize>, Vec<usize>) = round.iter().partition(|index| flag(**index));
    [(true, set), (false, unset)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_key_moves_to_the_next_round_in_order() {
        assert_eq!(rounds(["a", "b", "a", "c", "a"]), vec![vec![0, 1, 3], vec![2], vec![4]]);
        assert_eq!(rounds(Vec::<u8>::new()), Vec::<Vec<usize>>::new());
        assert_eq!(by_flag(&[0, 1, 2], |i| i != 1), [(true, vec![0, 2]), (false, vec![1])]);
    }
}
